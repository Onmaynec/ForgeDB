use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forgedb_core::{DatabaseId, Error, Limits, Result, SequenceNumber};
use forgedb_fs::NativeFileSystem;
use forgedb_manifest::ManifestState;
use forgedb_sstable::{TableMeta, TableReader};

use crate::table_scan::{self, TableSnapshotLookup};

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
    identity: DatabaseId,
    levels: Vec<Vec<TableHandle>>,
    max_file_size: u64,
}

impl VersionSet {
    pub(crate) fn open(
        directory: &Path,
        identity: DatabaseId,
        manifest: &ManifestState,
        limits: &Limits,
        level_count: usize,
        max_file_size: u64,
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
            let level_zero = levels.get_mut(0).ok_or(Error::InternalInvariant {
                invariant: "I-VERSIONSET-L0-PRESENT",
                reason: "VersionSet не содержит L0".to_owned(),
            })?;
            level_zero.push(TableHandle {
                metadata: metadata.clone(),
                reader: Arc::new(reader),
            });
        }
        let version = Self {
            directory: directory.to_path_buf(),
            identity,
            levels,
            max_file_size,
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

    pub(crate) fn get_at(
        &self,
        user_key: &[u8],
        snapshot: Option<SequenceNumber>,
        limits: &Limits,
        verify_checksums: bool,
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
            let lookup = table_scan::lookup_at(
                table.path(),
                self.identity,
                &table.metadata,
                user_key,
                snapshot,
                limits,
                self.max_file_size,
                verify_checksums,
            )?;
            if lookup != TableSnapshotLookup::Missing {
                return Ok(lookup);
            }
        }
        for level in self.levels.iter().skip(1) {
            let candidate = level
                .iter()
                .find(|table| table.metadata.may_contain_user_key(user_key));
            let Some(table) = candidate else {
                continue;
            };
            let lookup = table_scan::lookup_at(
                table.path(),
                self.identity,
                &table.metadata,
                user_key,
                snapshot,
                limits,
                self.max_file_size,
                verify_checksums,
            )?;
            if lookup != TableSnapshotLookup::Missing {
                return Ok(lookup);
            }
        }
        Ok(TableSnapshotLookup::Missing)
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

pub(crate) fn validate_level_metadata(
    level_index: usize,
    files: &[TableMeta],
    limits: &Limits,
) -> Result<()> {
    for file in files {
        file.validate(limits)?;
    }
    if level_index == 0 {
        if files
            .windows(2)
            .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(left, right)| {
                left.file_number >= right.file_number
            }))
        {
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

    fn metadata(
        number: u64,
        smallest: &[u8],
        largest: &[u8],
    ) -> Result<TableMeta> {
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
        let files = vec![
            metadata(10, b"a", b"m")?,
            metadata(11, b"m", b"z")?,
        ];
        assert!(validate_level_metadata(1, &files, &Limits::default()).is_err());
        Ok(())
    }

    #[test]
    fn l1_accepts_strictly_disjoint_ranges() -> Result<()> {
        let files = vec![
            metadata(10, b"a", b"m")?,
            metadata(11, b"n", b"z")?,
        ];
        validate_level_metadata(1, &files, &Limits::default())
    }

    #[test]
    fn l0_requires_creation_order() -> Result<()> {
        let files = vec![
            metadata(20, b"a", b"z")?,
            metadata(19, b"a", b"z")?,
        ];
        assert!(validate_level_metadata(0, &files, &Limits::default()).is_err());
        Ok(())
    }
}
