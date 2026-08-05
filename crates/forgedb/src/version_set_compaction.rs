/// Полностью подготовленная замена compaction inputs на уже проверенные output SSTables.
pub(crate) struct CompactionVersionRequest<'a> {
    pub(crate) source_level: usize,
    pub(crate) output_level: usize,
    pub(crate) source_files: &'a [u64],
    pub(crate) output_level_files: &'a [u64],
    pub(crate) smallest_user_key: &'a [u8],
    pub(crate) largest_user_key: &'a [u8],
    pub(crate) outputs: Vec<(TableMeta, TableReader)>,
    pub(crate) limits: &'a Limits,
}

impl VersionSet {
    /// Создаёт immutable VersionSet после compaction, не изменяя текущую Version.
    ///
    /// Метод удаляет только exact inputs выбранных levels. Уже открытые output
    /// readers устанавливаются только после повторной проверки metadata, key range
    /// и полного L0/L1+ layout. Физическое удаление inputs здесь запрещено.
    pub(crate) fn with_compaction(
        &self,
        request: CompactionVersionRequest<'_>,
    ) -> Result<Self> {
        let CompactionVersionRequest {
            source_level,
            output_level,
            source_files,
            output_level_files,
            smallest_user_key,
            largest_user_key,
            outputs,
            limits,
        } = request;

        if output_level != source_level.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "compaction_output_level",
            limit: u64::MAX,
            observed: u64::MAX,
        })? {
            return Err(Error::invalid_argument(
                "output_level",
                "compaction output level должен непосредственно следовать за source level",
            ));
        }
        if output_level >= self.levels.len() {
            return Err(Error::invalid_argument(
                "output_level",
                "compaction output level выходит за границы VersionSet",
            ));
        }
        if source_files.is_empty() {
            return Err(Error::invalid_argument(
                "source_files",
                "compaction требует минимум один source file",
            ));
        }
        if smallest_user_key > largest_user_key {
            return Err(Error::invalid_argument(
                "compaction_key_range",
                "smallest_user_key больше largest_user_key",
            ));
        }
        limits.validate_key(smallest_user_key)?;
        limits.validate_key(largest_user_key)?;

        let source_numbers = unique_numbers(source_files, "compaction source files")?;
        let target_numbers = unique_numbers(output_level_files, "compaction output-level inputs")?;
        if source_numbers
            .iter()
            .any(|number| target_numbers.contains(number))
        {
            return Err(Error::corruption(
                "compaction installation",
                "один file number указан одновременно в source и output-level inputs",
            ));
        }

        let current_numbers = self
            .levels
            .iter()
            .flat_map(|level| level.iter().map(|table| table.metadata.file_number))
            .collect::<BTreeSet<_>>();
        let mut output_numbers = BTreeSet::new();
        for (metadata, reader) in &outputs {
            metadata.validate(limits)?;
            if reader.meta() != metadata {
                return Err(Error::InternalInvariant {
                    invariant: "I-COMPACTION-OUTPUT-READER-METADATA",
                    reason: "output reader metadata не совпадает с prepared metadata".to_owned(),
                });
            }
            if metadata.smallest_user_key.as_slice() < smallest_user_key
                || metadata.largest_user_key.as_slice() > largest_user_key
            {
                return Err(Error::corruption(
                    "compaction installation",
                    "output SSTable выходит за key range выбранных inputs",
                ));
            }
            if !output_numbers.insert(metadata.file_number) {
                return Err(Error::corruption(
                    "compaction installation",
                    "duplicate output file number",
                ));
            }
            if current_numbers.contains(&metadata.file_number) {
                return Err(Error::corruption(
                    "compaction installation",
                    "output повторно использует live file number",
                ));
            }
        }

        let mut next = self.clone();
        remove_exact_inputs(
            next.levels
                .get_mut(source_level)
                .ok_or(Error::InternalInvariant {
                    invariant: "I-COMPACTION-SOURCE-LEVEL-PRESENT",
                    reason: "source level исчез из cloned VersionSet".to_owned(),
                })?,
            &source_numbers,
            "source level",
        )?;
        remove_exact_inputs(
            next.levels
                .get_mut(output_level)
                .ok_or(Error::InternalInvariant {
                    invariant: "I-COMPACTION-OUTPUT-LEVEL-PRESENT",
                    reason: "output level исчез из cloned VersionSet".to_owned(),
                })?,
            &target_numbers,
            "output level",
        )?;

        let target = next
            .levels
            .get_mut(output_level)
            .ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-OUTPUT-LEVEL-PRESENT",
                reason: "output level исчез до установки outputs".to_owned(),
            })?;
        target.extend(outputs.into_iter().map(|(metadata, reader)| TableHandle {
            metadata,
            reader: Arc::new(reader),
        }));
        target.sort_by(|left, right| {
            left.metadata
                .smallest_user_key
                .cmp(&right.metadata.smallest_user_key)
                .then_with(|| {
                    left.metadata
                        .largest_user_key
                        .cmp(&right.metadata.largest_user_key)
                })
                .then_with(|| left.metadata.file_number.cmp(&right.metadata.file_number))
        });
        next.validate(limits)?;
        Ok(next)
    }
}

fn unique_numbers(numbers: &[u64], component: &'static str) -> Result<BTreeSet<u64>> {
    let mut unique = BTreeSet::new();
    for number in numbers {
        if *number == 0 {
            return Err(Error::corruption(component, "file number равен нулю"));
        }
        if !unique.insert(*number) {
            return Err(Error::corruption(component, "duplicate file number"));
        }
    }
    Ok(unique)
}

fn remove_exact_inputs(
    level: &mut Vec<TableHandle>,
    expected: &BTreeSet<u64>,
    role: &'static str,
) -> Result<()> {
    let present = level
        .iter()
        .filter_map(|table| {
            expected
                .contains(&table.metadata.file_number)
                .then_some(table.metadata.file_number)
        })
        .collect::<BTreeSet<_>>();
    if &present != expected {
        let missing = expected
            .difference(&present)
            .copied()
            .collect::<Vec<_>>();
        return Err(Error::corruption(
            "compaction installation",
            format!("{role} не содержит exact selected inputs: missing {missing:?}"),
        ));
    }
    level.retain(|table| !expected.contains(&table.metadata.file_number));
    Ok(())
}

#[cfg(test)]
mod compaction_tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_manifest::{LevelFile, VersionEditV2};
    use forgedb_sstable::{
        TableBuildOptions, TableBuildRequest, TableEntry, build_table,
    };

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-version-compaction-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    fn build_reader(
        directory: &Path,
        identity: DatabaseId,
        file_number: u64,
        key: &[u8],
        sequence: u64,
        limits: &Limits,
    ) -> Result<(TableMeta, TableReader)> {
        let entries = vec![TableEntry::value(
            key.to_vec(),
            SequenceNumber::new(sequence)?,
            format!("value-{file_number}").into_bytes(),
            limits,
        )?];
        let filesystem = NativeFileSystem;
        let metadata = build_table(
            &filesystem,
            TableBuildRequest::new(
                directory,
                &directory.join("TEMP"),
                identity,
                file_number,
                &entries,
                limits,
                TableBuildOptions {
                    data_block_size: 1024,
                    restart_interval: 16,
                    bloom_bits_per_key: 10,
                },
            ),
        )?;
        let reader = TableReader::open(
            &filesystem,
            directory,
            identity,
            &metadata,
            limits,
            64 * 1024 * 1024,
        )?;
        Ok((metadata, reader))
    }

    #[test]
    fn exact_inputs_are_replaced_without_mutating_old_version() -> Result<()> {
        let directory = temporary_directory("replace")?;
        let identity = DatabaseId::random();
        let limits = Limits::default();
        let (l0_a, _) = build_reader(&directory, identity, 10, b"b", 4, &limits)?;
        let (l0_b, _) = build_reader(&directory, identity, 11, b"x", 3, &limits)?;
        let (l1_a, _) = build_reader(&directory, identity, 12, b"c", 2, &limits)?;
        let (l1_b, _) = build_reader(&directory, identity, 13, b"n", 1, &limits)?;
        let state = ManifestStateV2::initial(identity, 2, 1, 3)?.applied(
            &VersionEditV2 {
                next_file_number: Some(20),
                added_files: vec![
                    LevelFile {
                        level: 0,
                        metadata: l0_a,
                    },
                    LevelFile {
                        level: 0,
                        metadata: l0_b,
                    },
                    LevelFile {
                        level: 1,
                        metadata: l1_a,
                    },
                    LevelFile {
                        level: 1,
                        metadata: l1_b,
                    },
                ],
                ..VersionEditV2::default()
            },
            &limits,
        )?;
        let version = VersionSet::open_v2(
            &directory,
            identity,
            &state,
            &limits,
            64 * 1024 * 1024,
        )?;
        let (output_meta, output_reader) =
            build_reader(&directory, identity, 20, b"b", 4, &limits)?;
        let next = version.with_compaction(CompactionVersionRequest {
            source_level: 0,
            output_level: 1,
            source_files: &[10],
            output_level_files: &[12],
            smallest_user_key: b"b",
            largest_user_key: b"c",
            outputs: vec![(output_meta, output_reader)],
            limits: &limits,
        })?;

        assert_eq!(
            version.metadata_levels()[0]
                .iter()
                .map(|file| file.file_number)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
        assert_eq!(
            next.metadata_levels()[0]
                .iter()
                .map(|file| file.file_number)
                .collect::<Vec<_>>(),
            vec![11]
        );
        assert_eq!(
            next.metadata_levels()[1]
                .iter()
                .map(|file| file.file_number)
                .collect::<Vec<_>>(),
            vec![20, 13]
        );
        drop(next);
        drop(version);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn missing_selected_input_is_rejected() -> Result<()> {
        let directory = temporary_directory("missing")?;
        let identity = DatabaseId::random();
        let limits = Limits::default();
        let state = ManifestStateV2::initial(identity, 2, 1, 3)?;
        let version = VersionSet::open_v2(
            &directory,
            identity,
            &state,
            &limits,
            64 * 1024 * 1024,
        )?;
        assert!(matches!(
            version.with_compaction(CompactionVersionRequest {
                source_level: 0,
                output_level: 1,
                source_files: &[99],
                output_level_files: &[],
                smallest_user_key: b"a",
                largest_user_key: b"z",
                outputs: Vec::new(),
                limits: &limits,
            }),
            Err(Error::Corruption { .. })
        ));
        drop(version);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn output_overlap_with_unselected_l1_file_is_rejected() -> Result<()> {
        let directory = temporary_directory("overlap")?;
        let identity = DatabaseId::random();
        let limits = Limits::default();
        let (source_meta, _) = build_reader(&directory, identity, 10, b"n", 2, &limits)?;
        let (target_meta, _) = build_reader(&directory, identity, 11, b"n", 1, &limits)?;
        let state = ManifestStateV2::initial(identity, 2, 1, 3)?.applied(
            &VersionEditV2 {
                next_file_number: Some(20),
                added_files: vec![
                    LevelFile {
                        level: 0,
                        metadata: source_meta,
                    },
                    LevelFile {
                        level: 1,
                        metadata: target_meta,
                    },
                ],
                ..VersionEditV2::default()
            },
            &limits,
        )?;
        let version = VersionSet::open_v2(
            &directory,
            identity,
            &state,
            &limits,
            64 * 1024 * 1024,
        )?;
        let (output_meta, output_reader) =
            build_reader(&directory, identity, 20, b"n", 2, &limits)?;
        assert!(matches!(
            version.with_compaction(CompactionVersionRequest {
                source_level: 0,
                output_level: 1,
                source_files: &[10],
                output_level_files: &[],
                smallest_user_key: b"n",
                largest_user_key: b"n",
                outputs: vec![(output_meta, output_reader)],
                limits: &limits,
            }),
            Err(Error::Corruption { .. })
        ));
        drop(version);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
