const MAX_MANUAL_COMPACTION_STEPS: usize = 10_000;
const MANUAL_COMPACTION_LIMIT_U64: u64 = 10_000;
const MANUAL_COMPACTION_OBSERVED_U64: u64 = 10_001;

impl StreamingDatabase {
    /// Компактизирует все исходные source SSTables, пересекающие user-key range.
    ///
    /// Перед первым selection mutable state синхронно flush-ится. Затем каждый
    /// step использует полный durable installation protocol inner Database.
    /// Newly created outputs не добавляются в исходный work set: следующий
    /// manual вызов может продвинуть их дальше. Hard limit исключает бесконечный
    /// цикл при нарушении picker/install progress invariant.
    pub fn compact_range(&self, start: Option<&[u8]>, end: Option<&[u8]>) -> Result<()> {
        let limits = self.inner.options.limits()?;
        validate_compaction_range(start, end, &limits)?;
        self.inner.flush()?;

        let mut remaining = {
            let state = self.inner.lock_state()?;
            original_manual_sources(&state.version.metadata_levels(), start, end)?
        };
        if remaining.is_empty() {
            return Ok(());
        }

        for _ in 0..MAX_MANUAL_COMPACTION_STEPS {
            let before = {
                let state = self.inner.lock_state()?;
                source_file_layout(&state.version.metadata_levels())?
            };
            self.inner.compact_range(start, end)?;
            let after = {
                let state = self.inner.lock_state()?;
                source_file_layout(&state.version.metadata_levels())?
            };
            if before == after {
                return Err(Error::InternalInvariant {
                    invariant: "I-MANUAL-COMPACTION-PROGRESS",
                    reason: "manual picker не изменил source layout при наличии исходных inputs"
                        .to_owned(),
                });
            }

            let current_sources = after
                .iter()
                .flatten()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            remaining.retain(|file_number| current_sources.contains(file_number));
            if remaining.is_empty() {
                return Ok(());
            }
        }

        Err(Error::ResourceLimit {
            resource: "manual_compaction_steps",
            limit: MANUAL_COMPACTION_LIMIT_U64,
            observed: MANUAL_COMPACTION_OBSERVED_U64,
        })
    }
}

fn original_manual_sources(
    levels: &[Vec<forgedb_sstable::TableMeta>],
    start: Option<&[u8]>,
    end: Option<&[u8]>,
) -> Result<std::collections::BTreeSet<u64>> {
    let (_, source_levels) = levels.split_last().ok_or(Error::InternalInvariant {
        invariant: "I-MANUAL-COMPACTION-LEVELS",
        reason: "VersionSet не содержит levels".to_owned(),
    })?;
    Ok(source_levels
        .iter()
        .flatten()
        .filter(|file| manual_file_overlaps_range(file, start, end))
        .map(|file| file.file_number)
        .collect())
}

fn source_file_layout(levels: &[Vec<forgedb_sstable::TableMeta>]) -> Result<Vec<Vec<u64>>> {
    let (_, source_levels) = levels.split_last().ok_or(Error::InternalInvariant {
        invariant: "I-MANUAL-COMPACTION-LEVELS",
        reason: "VersionSet не содержит levels".to_owned(),
    })?;
    Ok(source_levels
        .iter()
        .map(|level| level.iter().map(|file| file.file_number).collect())
        .collect())
}

fn manual_file_overlaps_range(
    file: &forgedb_sstable::TableMeta,
    start: Option<&[u8]>,
    end: Option<&[u8]>,
) -> bool {
    if start.is_some_and(|start| file.largest_user_key.as_slice() < start) {
        return false;
    }
    if end.is_some_and(|end| file.smallest_user_key.as_slice() >= end) {
        return false;
    }
    true
}

#[cfg(test)]
mod exhaustive_compaction_wrapper_tests {
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
            "forgedb-exhaustive-compaction-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn one_manual_call_consumes_all_original_intersecting_l0_sources() -> Result<()> {
        let directory = temporary_directory("all-original-sources")?;
        let options = Options {
            l0_compaction_trigger: 10,
            l0_slowdown_trigger: 11,
            l0_stop_trigger: 12,
            target_sst_size: 1024,
            data_block_size: 1024,
            ..Options::default()
        };
        let database = StreamingDatabase::open(&directory, options)?;
        for (key, value) in [
            (b"alpha".as_slice(), b"one".as_slice()),
            (b"middle".as_slice(), b"two".as_slice()),
            (b"zulu".as_slice(), b"three".as_slice()),
        ] {
            database.put(key, value, WriteOptions::sync())?;
            database.flush()?;
        }

        database.compact_range(Some(b"a"), None)?;
        {
            let state = database.inner.lock_state()?;
            let levels = state.version.metadata_levels();
            let level_zero = levels.first().ok_or(Error::InternalInvariant {
                invariant: "I-EXHAUSTIVE-COMPACTION-L0",
                reason: "VersionSet не содержит L0".to_owned(),
            })?;
            let level_one = levels.get(1).ok_or(Error::InternalInvariant {
                invariant: "I-EXHAUSTIVE-COMPACTION-L1",
                reason: "VersionSet не содержит L1".to_owned(),
            })?;
            assert!(level_zero.is_empty());
            assert_eq!(level_one.len(), 3);
        }
        assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec()));
        assert_eq!(database.get(b"middle")?, Some(b"two".to_vec()));
        assert_eq!(database.get(b"zulu")?, Some(b"three".to_vec()));
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
