#[derive(Clone, Debug)]
struct CompactionPlan {
    target_level: usize,
    inputs: Vec<(usize, crate::version_set::TableHandle)>,
    bottommost: bool,
}

impl CompactionPlan {
    fn input_bytes(&self) -> Result<u64> {
        self.inputs.iter().try_fold(0_u64, |sum, (_, table)| {
            sum.checked_add(table.metadata().file_size)
                .ok_or(Error::ResourceLimit {
                    resource: "compaction_input_bytes",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })
        })
    }
}

impl Database {
    fn compact_once(
        &self,
        start: Option<&[u8]>,
        end: Option<&[u8]>,
        force: bool,
    ) -> Result<bool> {
        self.ensure_open()?;
        if self.options.read_only {
            return Err(Error::ReadOnly);
        }
        if let (Some(start), Some(end)) = (start, end)
            && start >= end
        {
            return Err(Error::invalid_argument(
                "compaction_range",
                "start должен быть строго меньше end",
            ));
        }

        let limits = self.options.limits()?;
        let (plan, oldest_snapshot) = {
            let mut state = self.lock_state()?;
            ensure_writer_healthy(&state)?;
            if state.flush_in_progress {
                return Err(Error::Busy {
                    reason: "flush или compaction уже выполняется".to_owned(),
                });
            }
            if state.pending_flush.is_some() {
                return Err(Error::Busy {
                    reason: "перед compaction требуется завершить pending flush".to_owned(),
                });
            }
            let Some(plan) = pick_compaction(
                state.version.as_ref(),
                &self.options,
                start,
                end,
                force,
            )? else {
                return Ok(false);
            };
            let input_bytes = plan.input_bytes()?;
            if input_bytes > self.options.max_compaction_bytes {
                return Err(Error::ResourceLimit {
                    resource: "compaction_input_bytes",
                    limit: self.options.max_compaction_bytes,
                    observed: input_bytes,
                });
            }
            state.flush_in_progress = true;
            (plan, self.snapshots.oldest_active_sequence()?)
        };

        let result = self.execute_compaction(&plan, oldest_snapshot, &limits);
        if result.is_err() {
            self.clear_flush_flag()?;
        }
        result.map(|()| true)
    }

    fn execute_compaction(
        &self,
        plan: &CompactionPlan,
        oldest_snapshot: Option<SequenceNumber>,
        limits: &Limits,
    ) -> Result<()> {
        let mut entries = Vec::<forgedb_sstable::TableEntry>::new();
        let mut resident_bytes = 0_u64;
        for (_, table) in &plan.inputs {
            let pool = self.lock_state()?.version.handle_pool();
            let table_entries = table_scan::read_entries_pooled(
                table.path(),
                self.identity,
                table.metadata(),
                limits,
                self.options.max_compaction_bytes,
                self.options.verify_checksums,
                pool,
            )?;
            for entry in table_entries {
                resident_bytes = resident_bytes
                    .checked_add(estimated_entry_bytes(&entry)?)
                    .ok_or(Error::ResourceLimit {
                        resource: "compaction_resident_bytes",
                        limit: self.options.max_compaction_bytes,
                        observed: u64::MAX,
                    })?;
                if resident_bytes > self.options.max_compaction_bytes {
                    return Err(Error::ResourceLimit {
                        resource: "compaction_resident_bytes",
                        limit: self.options.max_compaction_bytes,
                        observed: resident_bytes,
                    });
                }
                entries.push(entry);
            }
        }
        entries.sort_by(|left, right| left.internal_key.cmp(&right.internal_key));
        if entries
            .windows(2)
            .any(|pair| pair[0].internal_key == pair[1].internal_key)
        {
            return Err(Error::corruption(
                "compaction merge",
                "duplicate internal key обнаружен между input SSTables",
            ));
        }

        let chunks = build_compaction_chunks(
            entries,
            oldest_snapshot,
            plan.bottommost,
            self.options.target_sst_size,
        )?;
        let output_count = u64::try_from(chunks.len()).map_err(|_| Error::ResourceLimit {
            resource: "compaction_output_count",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;

        // File numbers резервируются durable MANIFEST edit до создания outputs. После crash
        // эти номера не переиспользуются, а успевшие появиться SST остаются unreferenced orphan.
        let first_file_number = {
            let mut state = self.lock_state()?;
            validate_compaction_inputs(&state, plan)?;
            let first = state.manifest.next_file_number;
            if output_count != 0 {
                let next = first
                    .checked_add(output_count)
                    .ok_or(Error::ResourceLimit {
                        resource: "file_number",
                        limit: u64::MAX,
                        observed: u64::MAX,
                    })?;
                let last_sequence = state.last_sequence;
                let reservation = VersionEdit {
                    next_file_number: Some(next),
                    last_sequence: Some(last_sequence),
                    ..VersionEdit::default()
                };
                append_manifest_edit(&mut state, &reservation, limits)?;
            }
            first
        };

        let filesystem = NativeFileSystem;
        let temporary_directory = self.directory.join(TEMP_DIRECTORY_NAME);
        let mut outputs = Vec::<(forgedb_sstable::TableMeta, TableReader)>::new();
        for (index, chunk) in chunks.iter().enumerate() {
            let offset = u64::try_from(index).map_err(|_| Error::ResourceLimit {
                resource: "compaction_output_index",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
            let file_number = first_file_number
                .checked_add(offset)
                .ok_or(Error::ResourceLimit {
                    resource: "file_number",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?;
            let metadata = build_table(
                &filesystem,
                TableBuildRequest::new(
                    &self.directory,
                    &temporary_directory,
                    self.identity,
                    file_number,
                    chunk,
                    limits,
                    TableBuildOptions {
                        data_block_size: self.options.data_block_size,
                        restart_interval: self.options.restart_interval,
                        bloom_bits_per_key: self.options.bloom_bits_per_key,
                    },
                ),
            )?;
            let reader = TableReader::open(
                &filesystem,
                &self.directory,
                self.identity,
                &metadata,
                limits,
                self.options.max_compaction_bytes,
            )?;
            outputs.push((metadata, reader));
        }

        let mut state = self.lock_state()?;
        validate_compaction_inputs(&state, plan)?;
        let deleted = plan
            .inputs
            .iter()
            .map(|(level, table)| (*level, table.metadata().file_number))
            .collect::<Vec<_>>();
        let added_for_version = outputs
            .iter()
            .map(|(metadata, reader)| (plan.target_level, metadata.clone(), reader.clone()))
            .collect::<Vec<_>>();
        let next_version = state
            .version
            .with_compaction(&deleted, added_for_version, limits)?;

        let last_sequence = state.last_sequence;
        let mut edit = VersionEdit {
            last_sequence: Some(last_sequence),
            ..VersionEdit::default()
        };
        for (level, table) in &plan.inputs {
            if *level == 0 {
                edit.deleted_files.push(table.metadata().file_number);
            } else {
                edit.deleted_level_files.push(forgedb_manifest::LevelFileRef::new(
                    u32::try_from(*level).map_err(|_| Error::ResourceLimit {
                        resource: "lsm_level",
                        limit: u64::from(u32::MAX),
                        observed: u64::MAX,
                    })?,
                    table.metadata().file_number,
                )?);
            }
        }
        for (metadata, _) in &outputs {
            edit.added_level_files.push(forgedb_manifest::LevelFileMeta::new(
                u32::try_from(plan.target_level).map_err(|_| Error::ResourceLimit {
                    resource: "lsm_level",
                    limit: u64::from(u32::MAX),
                    observed: u64::MAX,
                })?,
                metadata.clone(),
                limits,
            )?);
        }

        append_manifest_edit(&mut state, &edit, limits)?;

        // Только synced MANIFEST deletions ставим в очередь cleanup.
        // Пинним lifetime конкретного TableReader: он может разделяться несколькими
        // поколениями VersionSet, включая давно созданный streaming iterator.
        state.retired_sstables.extend(plan.inputs.iter().map(|(_, table)| {
            crate::obsolete::RetiredSstable {
                file_number: table.metadata().file_number,
                reader: table.reader_weak(),
            }
        }));
        state.version = Arc::new(next_version);
        state.flush_in_progress = false;
        Ok(())
    }

    fn l0_file_count(&self) -> Result<usize> {
        let state = self.lock_state()?;
        Ok(state.version.level_files(0)?.len())
    }
}

impl StreamingDatabase {
    /// Выполняет ручную leveled compaction указанного half-open user-key range.
    ///
    /// `None, None` означает полный keyspace. Перед compaction выполняется flush,
    /// поэтому committed mutable state участвует в compaction.
    pub fn compact_range(&self, start: Option<&[u8]>, end: Option<&[u8]>) -> Result<()> {
        self.inner.flush()?;
        while self.inner.compact_once(start, end, true)? {}
        self.try_cleanup_retired();
        Ok(())
    }

    fn compact_if_needed(&self) -> Result<()> {
        while self.inner.compact_once(None, None, false)? {}
        self.try_cleanup_retired();
        Ok(())
    }

    /// Очистка после durable MANIFEST installation не должна превращать уже
    /// завершённую compaction в ошибку публичной операции. Неудачные unlink
    /// остаются в очереди и доступны через явный cleanup с отчётом.
    fn try_cleanup_retired(&self) {
        let _ = self.inner.cleanup_obsolete_sstables();
    }

    fn apply_write_stall(&self) -> Result<()> {
        let l0_files = self.inner.l0_file_count()?;
        if l0_files >= self.inner.options.l0_stop_trigger {
            self.compact_if_needed()?;
            let remaining = self.inner.l0_file_count()?;
            if remaining >= self.inner.options.l0_stop_trigger {
                return Err(Error::Busy {
                    reason: format!(
                        "L0 stop threshold достигнут: {remaining} files при limit {}",
                        self.inner.options.l0_stop_trigger
                    ),
                });
            }
            return Ok(());
        }
        if l0_files >= self.inner.options.l0_slowdown_trigger {
            let excess = l0_files
                .saturating_sub(self.inner.options.l0_slowdown_trigger)
                .saturating_add(1);
            let delay_ms = u64::try_from(excess.min(10)).unwrap_or(10);
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
        Ok(())
    }
}

fn pick_compaction(
    version: &VersionSet,
    options: &Options,
    start: Option<&[u8]>,
    end: Option<&[u8]>,
    force: bool,
) -> Result<Option<CompactionPlan>> {
    if version.level_count() < 2 {
        return Err(Error::InternalInvariant {
            invariant: "I-COMPACTION-LEVEL-COUNT",
            reason: "compaction требует минимум два уровня".to_owned(),
        });
    }

    let l0 = version.level_files(0)?;
    if (force && l0.iter().any(|file| table_intersects(file.metadata(), start, end)))
        || (!force && l0.len() >= options.l0_compaction_trigger)
    {
        let seed = l0
            .iter()
            .find(|file| table_intersects(file.metadata(), start, end))
            .cloned();
        if let Some(seed) = seed {
            let mut selected = vec![seed];
            let mut smallest = selected[0].metadata().smallest_user_key.clone();
            let mut largest = selected[0].metadata().largest_user_key.clone();
            loop {
                let before = selected.len();
                for candidate in l0 {
                    if selected.iter().any(|current| {
                        current.metadata().file_number == candidate.metadata().file_number
                    }) {
                        continue;
                    }
                    if candidate.metadata().smallest_user_key.as_slice() <= largest.as_slice()
                        && smallest.as_slice() <= candidate.metadata().largest_user_key.as_slice()
                    {
                        smallest = smallest.min(candidate.metadata().smallest_user_key.clone());
                        largest = largest.max(candidate.metadata().largest_user_key.clone());
                        selected.push(candidate.clone());
                    }
                }
                if selected.len() == before {
                    break;
                }
            }
            selected.sort_by_key(|file| file.metadata().file_number);
            let mut inputs = selected
                .into_iter()
                .map(|file| (0_usize, file))
                .collect::<Vec<_>>();
            for file in version.overlapping_files(1, &smallest, &largest)? {
                inputs.push((1, file));
            }
            return Ok(Some(CompactionPlan {
                target_level: 1,
                inputs,
                bottommost: is_bottommost(version, 1, &smallest, &largest)?,
            }));
        }
    }

    for level in 1..version.level_count().saturating_sub(1) {
        let files = version.level_files(level)?;
        if files.is_empty() {
            continue;
        }
        let score_triggered = version.level_size(level)? >= level_target_size(options, level)?;
        if !force && !score_triggered {
            continue;
        }
        let Some(seed) = files
            .iter()
            .find(|file| table_intersects(file.metadata(), start, end))
            .cloned()
        else {
            continue;
        };
        let smallest = seed.metadata().smallest_user_key.clone();
        let largest = seed.metadata().largest_user_key.clone();
        let target_level = level.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "lsm_level",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        let mut inputs = vec![(level, seed)];
        for file in version.overlapping_files(target_level, &smallest, &largest)? {
            inputs.push((target_level, file));
        }
        return Ok(Some(CompactionPlan {
            target_level,
            inputs,
            bottommost: is_bottommost(version, target_level, &smallest, &largest)?,
        }));
    }
    Ok(None)
}

fn level_target_size(options: &Options, level: usize) -> Result<u64> {
    if level == 0 {
        return Err(Error::invalid_argument(
            "level",
            "L0 не использует byte-size target",
        ));
    }
    let mut target = options.level1_target_size;
    for _ in 1..level {
        target = target
            .checked_mul(options.level_size_multiplier)
            .ok_or(Error::ResourceLimit {
                resource: "level_target_size",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
    }
    Ok(target)
}

fn is_bottommost(
    version: &VersionSet,
    target_level: usize,
    smallest: &[u8],
    largest: &[u8],
) -> Result<bool> {
    for level in target_level.saturating_add(1)..version.level_count() {
        if !version.overlapping_files(level, smallest, largest)?.is_empty() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn table_intersects(
    metadata: &forgedb_sstable::TableMeta,
    start: Option<&[u8]>,
    end: Option<&[u8]>,
) -> bool {
    let after_start = start.is_none_or(|start| metadata.largest_user_key.as_slice() >= start);
    let before_end = end.is_none_or(|end| metadata.smallest_user_key.as_slice() < end);
    after_start && before_end
}

fn build_compaction_chunks(
    entries: Vec<forgedb_sstable::TableEntry>,
    oldest_snapshot: Option<SequenceNumber>,
    bottommost: bool,
    target_sst_size: u64,
) -> Result<Vec<Vec<forgedb_sstable::TableEntry>>> {
    let mut chunks = Vec::<Vec<forgedb_sstable::TableEntry>>::new();
    let mut current = Vec::<forgedb_sstable::TableEntry>::new();
    let mut current_bytes = 0_u64;
    let mut index = 0_usize;
    while index < entries.len() {
        let user_key = entries[index].internal_key.user_key().to_vec();
        let mut end = index.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "compaction_entry_index",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        while end < entries.len() && entries[end].internal_key.user_key() == user_key.as_slice() {
            end = end.checked_add(1).ok_or(Error::ResourceLimit {
                resource: "compaction_entry_index",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        }
        let retained = retain_versions(&entries[index..end], oldest_snapshot, bottommost)?;
        let group_bytes = retained.iter().try_fold(0_u64, |sum, entry| {
            sum.checked_add(estimated_entry_bytes(entry)?)
                .ok_or(Error::ResourceLimit {
                    resource: "compaction_output_bytes",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })
        })?;
        if !current.is_empty()
            && current_bytes
                .checked_add(group_bytes)
                .ok_or(Error::ResourceLimit {
                    resource: "compaction_output_bytes",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?
                > target_sst_size
        {
            chunks.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes
            .checked_add(group_bytes)
            .ok_or(Error::ResourceLimit {
                resource: "compaction_output_bytes",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        current.extend(retained);
        index = end;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    Ok(chunks)
}

fn retain_versions(
    group: &[forgedb_sstable::TableEntry],
    oldest_snapshot: Option<SequenceNumber>,
    bottommost: bool,
) -> Result<Vec<forgedb_sstable::TableEntry>> {
    let Some(newest) = group.first() else {
        return Ok(Vec::new());
    };
    if group
        .windows(2)
        .any(|pair| pair[0].internal_key.user_key() != pair[1].internal_key.user_key())
    {
        return Err(Error::InternalInvariant {
            invariant: "I-COMPACTION-USER-GROUP",
            reason: "retention получил несколько user keys".to_owned(),
        });
    }

    if bottommost
        && newest.internal_key.value_type() == forgedb_core::ValueType::Deletion
        && oldest_snapshot.is_none_or(|snapshot| newest.internal_key.sequence() <= snapshot)
    {
        return Ok(Vec::new());
    }

    let mut retained = vec![newest.clone()];
    let Some(oldest_snapshot) = oldest_snapshot else {
        return Ok(retained);
    };
    if newest.internal_key.sequence() <= oldest_snapshot {
        return Ok(retained);
    }
    for entry in group.iter().skip(1) {
        retained.push(entry.clone());
        if entry.internal_key.sequence() <= oldest_snapshot {
            break;
        }
    }
    Ok(retained)
}

fn estimated_entry_bytes(entry: &forgedb_sstable::TableEntry) -> Result<u64> {
    let key = u64::try_from(entry.internal_key.user_key().len()).map_err(|_| Error::ResourceLimit {
        resource: "compaction_key_bytes",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    let value = u64::try_from(entry.value.len()).map_err(|_| Error::ResourceLimit {
        resource: "compaction_value_bytes",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    key.checked_add(value)
        .and_then(|bytes| bytes.checked_add(64))
        .ok_or(Error::ResourceLimit {
            resource: "compaction_entry_bytes",
            limit: u64::MAX,
            observed: u64::MAX,
        })
}

fn validate_compaction_inputs(state: &DatabaseState, plan: &CompactionPlan) -> Result<()> {
    for (level, expected) in &plan.inputs {
        let still_live = state
            .version
            .level_files(*level)?
            .iter()
            .any(|table| table.metadata().file_number == expected.metadata().file_number);
        if !still_live {
            return Err(Error::Busy {
                reason: format!(
                    "compaction input SST-{:06} изменился до installation",
                    expected.metadata().file_number
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod compaction_tests {
    use super::*;

    fn entry(
        key: &[u8],
        sequence: u64,
        value_type: forgedb_core::ValueType,
    ) -> Result<forgedb_sstable::TableEntry> {
        let limits = Limits::default();
        match value_type {
            forgedb_core::ValueType::Value => forgedb_sstable::TableEntry::value(
                key.to_vec(),
                SequenceNumber::new(sequence)?,
                vec![u8::try_from(sequence % 251).unwrap_or(0)],
                &limits,
            ),
            forgedb_core::ValueType::Deletion => forgedb_sstable::TableEntry::tombstone(
                key.to_vec(),
                SequenceNumber::new(sequence)?,
                &limits,
            ),
        }
    }

    #[test]
    fn retention_keeps_snapshot_boundary_version() -> Result<()> {
        let group = vec![
            entry(b"k", 10, forgedb_core::ValueType::Value)?,
            entry(b"k", 8, forgedb_core::ValueType::Value)?,
            entry(b"k", 5, forgedb_core::ValueType::Value)?,
            entry(b"k", 2, forgedb_core::ValueType::Value)?,
        ];
        let retained = retain_versions(&group, Some(SequenceNumber::new(7)?), false)?;
        let sequences = retained
            .iter()
            .map(|item| item.internal_key.sequence().get())
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![10, 8, 5]);
        Ok(())
    }

    #[test]
    fn bottommost_tombstone_without_older_snapshot_is_dropped() -> Result<()> {
        let group = vec![
            entry(b"k", 10, forgedb_core::ValueType::Deletion)?,
            entry(b"k", 8, forgedb_core::ValueType::Value)?,
        ];
        assert!(retain_versions(&group, None, true)?.is_empty());
        Ok(())
    }

    #[test]
    fn output_split_never_divides_versions_of_one_user_key() -> Result<()> {
        let entries = vec![
            entry(b"a", 3, forgedb_core::ValueType::Value)?,
            entry(b"a", 2, forgedb_core::ValueType::Value)?,
            entry(b"b", 1, forgedb_core::ValueType::Value)?,
        ];
        let chunks = build_compaction_chunks(entries, Some(SequenceNumber::FIRST), false, 1)?;
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 2);
        assert!(chunks[0]
            .iter()
            .all(|entry| entry.internal_key.user_key() == b"a"));
        Ok(())
    }
}
