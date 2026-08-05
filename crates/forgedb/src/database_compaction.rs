impl forgedb_lsm::CompactionCursor for crate::sst_entry_cursor::SstEntryCursor {
    fn current(&self) -> Option<&forgedb_sstable::TableEntry> {
        crate::entry_cursor::EntryCursor::current(self)
    }

    fn advance(&mut self) -> Result<()> {
        crate::entry_cursor::EntryCursor::advance(self)
    }
}

impl Database {
    /// Выполняет одну eligible leveled compaction, пересекающую заданный user-key range.
    ///
    /// Первая production implementation синхронна и удерживает database state lock
    /// во время merge/build/install. Это намеренно сериализует writes, flush и close,
    /// пока следующий slice не добавит background ownership и write stalls.
    pub fn compact_range(&self, start: Option<&[u8]>, end: Option<&[u8]>) -> Result<()> {
        self.ensure_open()?;
        if self.options.read_only {
            return Err(Error::ReadOnly);
        }
        let limits = self.options.limits()?;
        validate_compaction_range(start, end, &limits)?;

        // Mutable state сначала переводится в установленный L0, чтобы compaction
        // работала только с immutable persisted inputs.
        self.flush()?;

        let filesystem = NativeFileSystem;
        let mut state = self.lock_state()?;
        ensure_writer_healthy(&state)?;
        if state.flush_in_progress {
            return Err(Error::Busy {
                reason: "нельзя начать compaction во время flush".to_owned(),
            });
        }

        let levels = state.version.metadata_levels();
        let decision = forgedb_lsm::pick_compaction(
            &levels,
            forgedb_lsm::CompactionConfig {
                l0_compaction_trigger: self.options.l0_compaction_trigger,
                level1_target_size: self.options.level1_target_size,
                level_size_multiplier: self.options.level_size_multiplier,
                max_compaction_bytes: self.options.max_compaction_bytes,
            },
        )?;
        let plan = match decision {
            forgedb_lsm::CompactionDecision::None => return Ok(()),
            forgedb_lsm::CompactionDecision::Blocked {
                plan,
                max_compaction_bytes,
            } => {
                return Err(Error::ResourceLimit {
                    resource: "compaction_input_bytes",
                    limit: max_compaction_bytes,
                    observed: plan.total_input_bytes,
                });
            }
            forgedb_lsm::CompactionDecision::Plan(plan) => plan,
        };
        if !plan_overlaps_requested_range(&plan, start, end) {
            return Ok(());
        }

        let inputs = selected_compaction_inputs(state.version.as_ref(), &plan)?;
        let mut cursors: Vec<Box<dyn forgedb_lsm::CompactionCursor>> =
            Vec::with_capacity(inputs.len());
        for (path, metadata) in &inputs {
            cursors.push(Box::new(crate::sst_entry_cursor::SstEntryCursor::open(
                path,
                self.identity,
                metadata,
                &limits,
                self.options.max_compaction_bytes,
                true,
            )?));
        }
        let oldest_snapshot = self.snapshots.oldest_active_sequence()?;
        let bottommost = plan
            .output_level
            .checked_add(1)
            .is_some_and(|next| next == self.options.level_count);
        let merge = forgedb_lsm::CompactionMerge::new(
            cursors,
            forgedb_lsm::CompactionMergeOptions {
                oldest_snapshot,
                bottommost,
                max_sources: inputs.len(),
                ..forgedb_lsm::CompactionMergeOptions::default()
            },
        )?;
        let splitter = forgedb_lsm::CompactionOutputSplitter::new(
            merge,
            forgedb_lsm::CompactionOutputOptions {
                target_file_size: self.options.target_sst_size,
                ..forgedb_lsm::CompactionOutputOptions::default()
            },
        )?;

        let mut outputs = Vec::<(forgedb_sstable::TableMeta, TableReader)>::new();
        let mut open_output: Option<(u64, Vec<forgedb_sstable::TableEntry>)> = None;
        for event in splitter {
            match event? {
                forgedb_lsm::CompactionOutputEvent::BeginFile { output_index } => {
                    if open_output.replace((output_index, Vec::new())).is_some() {
                        return Err(Error::InternalInvariant {
                            invariant: "I-COMPACTION-OUTPUT-SINGLE-OPEN",
                            reason: "splitter открыл второй output до завершения первого"
                                .to_owned(),
                        });
                    }
                }
                forgedb_lsm::CompactionOutputEvent::Entry(entry) => {
                    open_output
                        .as_mut()
                        .ok_or(Error::InternalInvariant {
                            invariant: "I-COMPACTION-OUTPUT-OPEN",
                            reason: "entry получена без открытого output".to_owned(),
                        })?
                        .1
                        .push(entry);
                }
                forgedb_lsm::CompactionOutputEvent::FinishFile(summary) => {
                    let (output_index, entries) =
                        open_output.take().ok_or(Error::InternalInvariant {
                            invariant: "I-COMPACTION-OUTPUT-FINISH",
                            reason: "finish получен без открытого output".to_owned(),
                        })?;
                    if output_index != summary.output_index {
                        return Err(Error::InternalInvariant {
                            invariant: "I-COMPACTION-OUTPUT-INDEX",
                            reason: "begin/finish output index не совпадают".to_owned(),
                        });
                    }
                    let entry_count = u64::try_from(entries.len()).map_err(|_| {
                        Error::ResourceLimit {
                            resource: "compaction_output_entries",
                            limit: u64::MAX,
                            observed: u64::MAX,
                        }
                    })?;
                    if entries.is_empty() || entry_count != summary.entry_count {
                        return Err(Error::InternalInvariant {
                            invariant: "I-COMPACTION-OUTPUT-ENTRY-COUNT",
                            reason: "фактическое число output entries не совпадает с summary"
                                .to_owned(),
                        });
                    }
                    let file_number = self.reserve_compaction_file_number(
                        &mut state,
                        &filesystem,
                        &limits,
                    )?;
                    let metadata = build_table(
                        &filesystem,
                        TableBuildRequest::new(
                            &self.directory,
                            &self.directory.join(TEMP_DIRECTORY_NAME),
                            self.identity,
                            file_number,
                            &entries,
                            &limits,
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
                        &limits,
                        self.options.max_compaction_bytes,
                    )?;
                    outputs.push((metadata, reader));
                }
            }
        }
        if open_output.is_some() {
            return Err(Error::InternalInvariant {
                invariant: "I-COMPACTION-OUTPUT-CLOSED",
                reason: "splitter завершился с незакрытым output".to_owned(),
            });
        }

        let output_metadata = outputs
            .iter()
            .map(|(metadata, _)| metadata.clone())
            .collect::<Vec<_>>();
        let previous_version = Arc::clone(&state.version);
        let next_version = previous_version.with_compaction(
            crate::version_set::CompactionVersionRequest {
                source_level: plan.source_level,
                output_level: plan.output_level,
                source_files: &plan.source_files,
                output_level_files: &plan.output_level_files,
                smallest_user_key: &plan.smallest_user_key,
                largest_user_key: &plan.largest_user_key,
                outputs,
                limits: &limits,
            },
        )?;
        let source_level = checked_manifest_level(plan.source_level)?;
        let output_level = checked_manifest_level(plan.output_level)?;
        let mut deleted_files = plan
            .source_files
            .iter()
            .map(|file_number| forgedb_manifest::DeletedLevelFile {
                level: source_level,
                file_number: *file_number,
            })
            .collect::<Vec<_>>();
        deleted_files.extend(plan.output_level_files.iter().map(|file_number| {
            forgedb_manifest::DeletedLevelFile {
                level: output_level,
                file_number: *file_number,
            }
        }));
        let obsolete_files = plan
            .source_files
            .iter()
            .chain(plan.output_level_files.iter())
            .copied()
            .collect::<Vec<_>>();
        let installation = VersionEditV2 {
            last_sequence: Some(state.last_sequence),
            added_files: output_metadata
                .into_iter()
                .map(|metadata| LevelFile {
                    level: output_level,
                    metadata,
                })
                .collect(),
            deleted_files,
            compaction_pointers: vec![forgedb_manifest::CompactionPointer {
                level: source_level,
                user_key: plan.largest_user_key,
            }],
            ..VersionEditV2::default()
        };
        append_manifest_edit(&mut state, &installation, &limits)?;
        let obsolete = ObsoleteVersion::new(&previous_version, obsolete_files)?;
        state.version = Arc::new(next_version);
        state.obsolete_versions.push(obsolete);
        drop(previous_version);
        self.collect_obsolete_ssts_locked(&mut state)?;
        Ok(())
    }

    fn reserve_compaction_file_number(
        &self,
        state: &mut DatabaseState,
        _filesystem: &NativeFileSystem,
        limits: &Limits,
    ) -> Result<u64> {
        let file_number = state.manifest.next_file_number;
        let final_path = self.directory.join(sst_file_name(file_number));
        let temporary_path = self
            .directory
            .join(TEMP_DIRECTORY_NAME)
            .join(forgedb_sstable::temporary_sst_file_name(file_number));
        ensure_compaction_path_absent(&final_path, "compaction output SSTable")?;
        ensure_compaction_path_absent(&temporary_path, "compaction temporary SSTable")?;
        let next_file_number = file_number.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "file_number",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        append_manifest_edit(
            state,
            &VersionEditV2 {
                next_file_number: Some(next_file_number),
                ..VersionEditV2::default()
            },
            limits,
        )?;
        Ok(file_number)
    }
}

fn validate_compaction_range(
    start: Option<&[u8]>,
    end: Option<&[u8]>,
    limits: &Limits,
) -> Result<()> {
    if let Some(start) = start {
        limits.validate_key(start)?;
    }
    if let Some(end) = end {
        limits.validate_key(end)?;
    }
    if start.zip(end).is_some_and(|(start, end)| start > end) {
        return Err(Error::invalid_argument(
            "compaction_range",
            "start больше end",
        ));
    }
    Ok(())
}

fn plan_overlaps_requested_range(
    plan: &forgedb_lsm::CompactionPlan,
    start: Option<&[u8]>,
    end: Option<&[u8]>,
) -> bool {
    if start.is_some_and(|start| plan.largest_user_key.as_slice() < start) {
        return false;
    }
    if end.is_some_and(|end| plan.smallest_user_key.as_slice() >= end) {
        return false;
    }
    true
}

fn selected_compaction_inputs(
    version: &VersionSet,
    plan: &forgedb_lsm::CompactionPlan,
) -> Result<Vec<(PathBuf, forgedb_sstable::TableMeta)>> {
    let levels = version.metadata_levels();
    let mut inputs = Vec::with_capacity(
        plan.source_files
            .len()
            .checked_add(plan.output_level_files.len())
            .ok_or(Error::ResourceLimit {
                resource: "compaction_input_files",
                limit: u64::MAX,
                observed: u64::MAX,
            })?,
    );
    append_selected_level_inputs(
        version,
        &levels,
        plan.source_level,
        &plan.source_files,
        &mut inputs,
    )?;
    append_selected_level_inputs(
        version,
        &levels,
        plan.output_level,
        &plan.output_level_files,
        &mut inputs,
    )?;
    Ok(inputs)
}

fn append_selected_level_inputs(
    version: &VersionSet,
    levels: &[Vec<forgedb_sstable::TableMeta>],
    level: usize,
    selected: &[u64],
    output: &mut Vec<(PathBuf, forgedb_sstable::TableMeta)>,
) -> Result<()> {
    let metadata = levels.get(level).ok_or(Error::InternalInvariant {
        invariant: "I-COMPACTION-INPUT-LEVEL",
        reason: "selected compaction level отсутствует".to_owned(),
    })?;
    let mut unique = std::collections::BTreeSet::new();
    for file_number in selected {
        if !unique.insert(*file_number) {
            return Err(Error::corruption(
                "compaction plan",
                "duplicate input file number",
            ));
        }
        let file = metadata
            .iter()
            .find(|file| file.file_number == *file_number)
            .ok_or_else(|| {
                Error::corruption(
                    "compaction plan",
                    format!("selected input SST-{file_number:06} отсутствует в level {level}"),
                )
            })?
            .clone();
        output.push((
            version.directory().join(sst_file_name(file.file_number)),
            file,
        ));
    }
    Ok(())
}

fn checked_manifest_level(level: usize) -> Result<u32> {
    u32::try_from(level).map_err(|_| Error::ResourceLimit {
        resource: "manifest_level",
        limit: u64::from(u32::MAX),
        observed: u64::MAX,
    })
}

fn ensure_compaction_path_absent(path: &Path, role: &'static str) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(_) => Err(Error::corruption(
            role,
            format!("destination уже существует: {}", path.display()),
        )),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::io(
            "stat_compaction_output",
            Some(path.to_path_buf()),
            source,
        )),
    }
}

#[cfg(test)]
mod database_compaction_tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-database-compaction-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    fn compaction_options() -> Options {
        Options {
            target_sst_size: 1024,
            data_block_size: 1024,
            l0_compaction_trigger: 2,
            l0_slowdown_trigger: 4,
            l0_stop_trigger: 6,
            max_compaction_bytes: 64 * 1024 * 1024,
            ..Options::default()
        }
    }

    #[test]
    fn physical_compaction_preserves_latest_snapshot_and_reopen() -> Result<()> {
        let directory = temporary_directory("snapshot-reopen")?;
        let database = Database::open(&directory, compaction_options())?;
        database.put(b"alpha", b"old", WriteOptions::sync())?;
        let snapshot = database.snapshot()?;
        database.flush()?;
        database.put(b"alpha", b"new", WriteOptions::sync())?;
        database.put(b"beta", b"value", WriteOptions::sync())?;
        database.flush()?;

        assert_eq!(database.lock_state()?.version.metadata_levels()[0].len(), 2);
        database.compact_range(None, None)?;
        assert_eq!(database.get(b"alpha")?, Some(b"new".to_vec()));
        assert_eq!(
            database.get_with_options(b"alpha", ReadOptions::from_snapshot(&snapshot))?,
            Some(b"old".to_vec())
        );
        {
            let state = database.lock_state()?;
            let levels = state.version.metadata_levels();
            assert!(levels[0].is_empty());
            assert_eq!(levels[1].len(), 1);
        }
        let inventory = database.file_inventory()?;
        assert_eq!(inventory.live_sst.len(), 1);
        assert!(inventory.orphan_sst.is_empty());
        drop(snapshot);
        database.close()?;

        let reopened = Database::open(&directory, compaction_options())?;
        assert_eq!(reopened.get(b"alpha")?, Some(b"new".to_vec()));
        assert_eq!(reopened.get(b"beta")?, Some(b"value".to_vec()));
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn missing_installed_compaction_output_blocks_reopen() -> Result<()> {
        let directory = temporary_directory("missing-output")?;
        let database = Database::open(&directory, compaction_options())?;
        database.put(b"alpha", b"one", WriteOptions::sync())?;
        database.flush()?;
        database.put(b"alpha", b"two", WriteOptions::sync())?;
        database.flush()?;
        database.compact_range(None, None)?;
        let live = database.file_inventory()?.live_sst;
        assert_eq!(live.len(), 1);
        database.close()?;
        let live_path = directory.join(live.first().ok_or(Error::InternalInvariant {
            invariant: "I-COMPACTION-TEST-LIVE-SST",
            reason: "inventory не содержит установленный output".to_owned(),
        })?);
        fs::remove_file(&live_path)
            .map_err(|source| Error::io("remove_compaction_output", Some(live_path), source))?;
        assert!(matches!(
            Database::open(&directory, compaction_options()),
            Err(Error::Io { .. }) | Err(Error::Corruption { .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn active_streaming_scan_defers_input_deletion_until_iterator_drop() -> Result<()> {
        let directory = temporary_directory("active-scan-lifetime")?;
        let database = StreamingDatabase::open(&directory, compaction_options())?;
        database.put(b"alpha", b"old", WriteOptions::sync())?;
        database.flush()?;
        database.put(b"alpha", b"new", WriteOptions::sync())?;
        database.put(b"beta", b"value", WriteOptions::sync())?;
        database.flush()?;

        let iterator = database.scan(.., ReadOptions::default())?;
        database.compact_range(None, None)?;
        assert_eq!(database.file_inventory()?.orphan_sst.len(), 2);

        drop(iterator);
        assert_eq!(database.file_inventory()?.orphan_sst.len(), 2);
        database.put(b"cleanup-trigger", b"value", WriteOptions::sync())?;
        assert!(database.file_inventory()?.orphan_sst.is_empty());
        database.close()?;
        fs::remove_dir_all(&directory).map_err(|source| {
            Error::io("remove_test_directory", Some(directory), source)
        })?;
        Ok(())
    }
}
