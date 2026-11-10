use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forgedb_core::{DatabaseId, Error, Limits, Result, SequenceNumber};
use forgedb_fs::NativeFileSystem;
use forgedb_manifest::ManifestState;
use forgedb_sstable::{BlockCache, CacheStats, TableLookup, TableMeta, TableReader};

use crate::table_scan::TableSnapshotLookup;

#[derive(Clone, Debug)]
pub(crate) struct TableHandle {
    metadata: TableMeta,
    reader: Arc<TableReader>,
}

impl TableHandle {
    #[must_use]
    pub(crate) fn metadata(&self) -> &TableMeta {
        &self.metadata
    }

    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        self.reader.path()
    }
}

/// Immutable snapshot структуры LSM. Readers удерживают `Arc<VersionSet>` без writer mutex.
#[derive(Clone, Debug)]
pub(crate) struct VersionSet {
    directory: PathBuf,
    levels: Vec<Vec<TableHandle>>,
    block_cache: Arc<BlockCache>,
}

impl VersionSet {
    pub(crate) fn open(
        directory: &Path,
        identity: DatabaseId,
        manifest: &ManifestState,
        limits: &Limits,
        level_count: usize,
        max_file_size: u64,
        block_cache_size: usize,
    ) -> Result<Self> {
        if level_count < 2 {
            return Err(Error::invalid_argument(
                "level_count",
                "VersionSet требует минимум L0 и L1",
            ));
        }
        let filesystem = NativeFileSystem;
        let mut levels = (0..level_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for metadata in &manifest.l0_files {
            let reader = TableReader::open(
                &filesystem,
                directory,
                identity,
                metadata,
                limits,
                max_file_size,
            )?;
            levels[0].push(TableHandle {
                metadata: metadata.clone(),
                reader: Arc::new(reader),
            });
        }
        for leveled in &manifest.level_files {
            let level = usize::try_from(leveled.level).map_err(|_| Error::ResourceLimit {
                resource: "lsm_level",
                limit: u64::try_from(level_count.saturating_sub(1)).unwrap_or(u64::MAX),
                observed: u64::from(leveled.level),
            })?;
            if level == 0 || level >= level_count {
                return Err(Error::corruption(
                    "VersionSet",
                    format!("MANIFEST содержит SSTable вне configured level_count: L{level}"),
                ));
            }
            let reader = TableReader::open(
                &filesystem,
                directory,
                identity,
                &leveled.table,
                limits,
                max_file_size,
            )?;
            levels[level].push(TableHandle {
                metadata: leveled.table.clone(),
                reader: Arc::new(reader),
            });
        }
        let version = Self {
            directory: directory.to_path_buf(),
            levels,
            block_cache: Arc::new(BlockCache::new(block_cache_size)),
        };
        version.validate(limits)?;
        Ok(version)
    }

    pub(crate) fn with_l0(
        &self,
        metadata: TableMeta,
        reader: TableReader,
        limits: &Limits,
    ) -> Result<Self> {
        let mut next = self.clone();
        let level_zero = next.levels.get_mut(0).ok_or(Error::InternalInvariant {
            invariant: "I-VERSIONSET-L0-PRESENT",
            reason: "VersionSet не содержит L0".to_owned(),
        })?;
        level_zero.push(TableHandle {
            metadata,
            reader: Arc::new(reader),
        });
        next.validate(limits)?;
        Ok(next)
    }

    pub(crate) fn with_compaction(
        &self,
        deleted: &[(usize, u64)],
        added: Vec<(usize, TableMeta, TableReader)>,
        limits: &Limits,
    ) -> Result<Self> {
        let mut next = self.clone();
        for (level, file_number) in deleted {
            let files = next
                .levels
                .get_mut(*level)
                .ok_or(Error::InternalInvariant {
                    invariant: "I-COMPACTION-DELETE-LEVEL",
                    reason: format!("уровень L{level} отсутствует"),
                })?;
            let before = files.len();
            files.retain(|file| file.metadata.file_number != *file_number);
            if files.len() == before {
                return Err(Error::InternalInvariant {
                    invariant: "I-COMPACTION-DELETE-LIVE-FILE",
                    reason: format!("SST-{file_number:06} отсутствует в L{level}"),
                });
            }
        }
        for (level, metadata, reader) in added {
            if level == 0 {
                return Err(Error::InternalInvariant {
                    invariant: "I-COMPACTION-OUTPUT-LEVEL",
                    reason: "compaction output не устанавливается обратно в L0".to_owned(),
                });
            }
            let files = next.levels.get_mut(level).ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-OUTPUT-LEVEL",
                reason: format!("уровень L{level} отсутствует"),
            })?;
            files.push(TableHandle {
                metadata,
                reader: Arc::new(reader),
            });
            files.sort_by(|left, right| {
                left.metadata
                    .smallest_user_key
                    .cmp(&right.metadata.smallest_user_key)
                    .then_with(|| left.metadata.file_number.cmp(&right.metadata.file_number))
            });
        }
        next.validate(limits)?;
        Ok(next)
    }

    pub(crate) fn get_at(
        &self,
        user_key: &[u8],
        snapshot: Option<SequenceNumber>,
        _limits: &Limits,
        _verify_checksums: bool,
    ) -> Result<TableSnapshotLookup> {
        let Some(level_zero) = self.levels.first() else {
            return Err(Error::InternalInvariant {
                invariant: "I-VERSIONSET-L0-PRESENT",
                reason: "VersionSet не содержит L0".to_owned(),
            });
        };
        for table in level_zero.iter().rev() {
            if !table.metadata.may_contain_user_key(user_key) {
                continue;
            }
            let lookup = table
                .reader
                .get_at_cached(user_key, snapshot, &self.block_cache)?;
            if lookup != TableLookup::Missing {
                return Ok(convert_lookup(lookup));
            }
        }
        for level in self.levels.iter().skip(1) {
            // L1+ ranges не пересекаются: binary search вместо линейного обхода уровня.
            let index = level
                .partition_point(|table| table.metadata.largest_user_key.as_slice() < user_key);
            let Some(table) = level
                .get(index)
                .filter(|table| table.metadata.may_contain_user_key(user_key))
            else {
                continue;
            };
            let lookup = table
                .reader
                .get_at_cached(user_key, snapshot, &self.block_cache)?;
            if lookup != TableLookup::Missing {
                return Ok(convert_lookup(lookup));
            }
        }
        Ok(TableSnapshotLookup::Missing)
    }

    /// Проверка write-write conflict использует SSTable index и Bloom filter:
    /// полная материализация всех entries таблицы больше не требуется.
    pub(crate) fn latest_sequence_after(
        &self,
        user_key: &[u8],
        after: Option<SequenceNumber>,
        _limits: &Limits,
        _verify_checksums: bool,
    ) -> Result<Option<SequenceNumber>> {
        let mut newest: Option<SequenceNumber> = None;
        let threshold = after.map(SequenceNumber::get).unwrap_or(0);
        for level in &self.levels {
            for table in level {
                if !table.metadata.may_contain_user_key(user_key)
                    || table.metadata.largest_sequence.get() <= threshold
                {
                    continue;
                }
                if let Some(sequence) =
                    table
                        .reader
                        .latest_sequence_after_cached(user_key, after, &self.block_cache)?
                {
                    newest = Some(newest.map_or(sequence, |current| current.max(sequence)));
                }
            }
        }
        Ok(newest)
    }

    pub(crate) fn cache_stats(&self) -> Result<CacheStats> {
        self.block_cache.stats()
    }

    pub(crate) fn is_live_file(&self, file_number: u64) -> bool {
        self.levels
            .iter()
            .flatten()
            .any(|table| table.metadata.file_number == file_number)
    }

    pub(crate) fn evict_cached_table(&self, file_number: u64) -> Result<()> {
        self.block_cache.evict_table(file_number)
    }

    pub(crate) fn files_in_read_order(&self) -> Result<Vec<TableHandle>> {
        let level_zero = self.levels.first().ok_or(Error::InternalInvariant {
            invariant: "I-VERSIONSET-L0-PRESENT",
            reason: "VersionSet не содержит L0".to_owned(),
        })?;
        let mut files = Vec::with_capacity(self.file_count());
        files.extend(level_zero.iter().rev().cloned());
        for level in self.levels.iter().skip(1) {
            files.extend(level.iter().cloned());
        }
        Ok(files)
    }

    pub(crate) fn level_files(&self, level: usize) -> Result<&[TableHandle]> {
        self.levels.get(level).map(Vec::as_slice).ok_or_else(|| {
            Error::invalid_argument("level", format!("уровень L{level} отсутствует"))
        })
    }

    pub(crate) fn overlapping_files(
        &self,
        level: usize,
        smallest: &[u8],
        largest: &[u8],
    ) -> Result<Vec<TableHandle>> {
        if smallest > largest {
            return Err(Error::invalid_argument(
                "compaction_range",
                "smallest key больше largest key",
            ));
        }
        Ok(self
            .level_files(level)?
            .iter()
            .filter(|table| {
                table.metadata.smallest_user_key.as_slice() <= largest
                    && smallest <= table.metadata.largest_user_key.as_slice()
            })
            .cloned()
            .collect())
    }

    pub(crate) fn level_size(&self, level: usize) -> Result<u64> {
        self.level_files(level)?
            .iter()
            .try_fold(0_u64, |sum, table| {
                sum.checked_add(table.metadata.file_size)
                    .ok_or(Error::ResourceLimit {
                        resource: "level_size",
                        limit: u64::MAX,
                        observed: u64::MAX,
                    })
            })
    }

    pub(crate) fn validate(&self, limits: &Limits) -> Result<()> {
        if self.levels.len() < 2 {
            return Err(Error::corruption(
                "VersionSet",
                "должно существовать минимум два уровня",
            ));
        }
        let mut file_numbers = BTreeSet::new();
        for (level_index, level) in self.levels.iter().enumerate() {
            let metadata = level
                .iter()
                .map(|table| table.metadata.clone())
                .collect::<Vec<_>>();
            validate_level_metadata(level_index, &metadata, limits)?;
            for file in metadata {
                if !file_numbers.insert(file.file_number) {
                    return Err(Error::corruption(
                        "VersionSet",
                        "duplicate file number между уровнями",
                    ));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub(crate) fn file_count(&self) -> usize {
        self.levels.iter().map(Vec::len).sum()
    }

    #[must_use]
    pub(crate) fn level_count(&self) -> usize {
        self.levels.len()
    }

    #[must_use]
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }
}

pub(crate) fn convert_lookup(lookup: TableLookup) -> TableSnapshotLookup {
    match lookup {
        TableLookup::Missing => TableSnapshotLookup::Missing,
        TableLookup::Tombstone => TableSnapshotLookup::Tombstone,
        TableLookup::Value(value) => TableSnapshotLookup::Value(value),
    }
}

fn validate_level_metadata(level_index: usize, files: &[TableMeta], limits: &Limits) -> Result<()> {
    for file in files {
        file.validate(limits)?;
    }
    if level_index == 0 {
        if files.windows(2).any(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left.file_number >= right.file_number)
        }) {
            return Err(Error::corruption(
                "VersionSet L0",
                "L0 file numbers не строго возрастают",
            ));
        }
        return Ok(());
    }

    for pair in files.windows(2) {
        let left = pair.first().ok_or(Error::InternalInvariant {
            invariant: "I-LEVEL-WINDOW-LEFT",
            reason: "level window не содержит left file".to_owned(),
        })?;
        let right = pair.get(1).ok_or(Error::InternalInvariant {
            invariant: "I-LEVEL-WINDOW-RIGHT",
            reason: "level window не содержит right file".to_owned(),
        })?;
        if left.smallest_user_key >= right.smallest_user_key {
            return Err(Error::corruption(
                "VersionSet L1+",
                "файлы не отсортированы по smallest key",
            ));
        }
        if left.largest_user_key >= right.smallest_user_key {
            return Err(Error::corruption(
                "VersionSet L1+",
                "key ranges внутри уровня перекрываются",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(number: u64, smallest: &[u8], largest: &[u8]) -> Result<TableMeta> {
        Ok(TableMeta {
            file_number: number,
            file_size: 256,
            smallest_user_key: smallest.to_vec(),
            largest_user_key: largest.to_vec(),
            smallest_sequence: SequenceNumber::FIRST,
            largest_sequence: SequenceNumber::FIRST,
            entry_count: 1,
        })
    }

    #[test]
    fn l1_rejects_overlapping_ranges() -> Result<()> {
        let files = vec![metadata(10, b"a", b"m")?, metadata(11, b"m", b"z")?];
        assert!(validate_level_metadata(1, &files, &Limits::default()).is_err());
        Ok(())
    }

    #[test]
    fn l1_accepts_strictly_disjoint_ranges() -> Result<()> {
        let files = vec![metadata(10, b"a", b"m")?, metadata(11, b"n", b"z")?];
        validate_level_metadata(1, &files, &Limits::default())
    }

    #[test]
    fn l0_requires_creation_order() -> Result<()> {
        let files = vec![metadata(20, b"a", b"z")?, metadata(19, b"a", b"z")?];
        assert!(validate_level_metadata(0, &files, &Limits::default()).is_err());
        Ok(())
    }
}
