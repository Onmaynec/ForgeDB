use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forgedb_core::{DatabaseId, Error, Limits, Result, SequenceNumber};
use forgedb_fs::NativeFileSystem;
use forgedb_manifest::ManifestStateV2;
use forgedb_sstable::{TableMeta, TableReader};

use crate::table_scan::{self, TableLookupRequest, TableSnapshotLookup};

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
    pub(crate) fn open_v2(
        directory: &Path,
        identity: DatabaseId,
        manifest: &ManifestStateV2,
        limits: &Limits,
        max_file_size: u64,
    ) -> Result<Self> {
        manifest.validate(limits)?;
        let filesystem = NativeFileSystem;
        let mut levels = Vec::with_capacity(manifest.levels.len());
        for metadata_level in &manifest.levels {
            let mut level = Vec::with_capacity(metadata_level.len());
            for metadata in metadata_level {
                let reader = TableReader::open(
                    &filesystem,
                    directory,
                    identity,
                    metadata,
                    limits,
                    max_file_size,
                )?;
                level.push(TableHandle {
                    metadata: metadata.clone(),
                    reader: Arc::new(reader),
                });
            }
            levels.push(level);
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
            let lookup = table_scan::lookup_at(TableLookupRequest {
                path: table.path(),
                expected_database_id: self.identity,
                metadata: &table.metadata,
                user_key,
                snapshot,
                limits,
                max_file_size: self.max_file_size,
                verify_checksums,
            })?;
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
            let lookup = table_scan::lookup_at(TableLookupRequest {
                path: table.path(),
                expected_database_id: self.identity,
                metadata: &table.metadata,
                user_key,
                snapshot,
                limits,
                max_file_size: self.max_file_size,
                verify_checksums,
            })?;
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
    pub(crate) fn metadata_levels(&self) -> Vec<Vec<TableMeta>> {
        self.levels
            .iter()
            .map(|level| level.iter().map(|table| table.metadata.clone()).collect())
            .collect()
    }

    #[must_use]
    pub(crate) fn file_count(&self) -> usize {
        self.metadata_levels().iter().map(Vec::len).sum()
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

    #[test]
    fn v2_version_set_opens_physical_l0_and_l1_files() -> Result<()> {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        use forgedb_manifest::{LevelFile, VersionEditV2};
        use forgedb_sstable::{TableBuildOptions, TableBuildRequest, TableEntry, build_table};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "forgedb-version-set-v2-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).map_err(|source| {
            Error::io("create_test_directory", Some(directory.clone()), source)
        })?;
        let temporary = directory.join("TEMP");
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let identity = DatabaseId::random();
        let options = TableBuildOptions {
            data_block_size: 1024,
            restart_interval: 16,
            bloom_bits_per_key: 10,
        };
        let l0_entries = vec![TableEntry::value(
            b"a".to_vec(),
            SequenceNumber::new(2)?,
            b"l0".to_vec(),
            &limits,
        )?];
        let l1_entries = vec![TableEntry::value(
            b"n".to_vec(),
            SequenceNumber::FIRST,
            b"l1".to_vec(),
            &limits,
        )?];
        let l0 = build_table(
            &filesystem,
            TableBuildRequest::new(
                &directory,
                &temporary,
                identity,
                10,
                &l0_entries,
                &limits,
                options,
            ),
        )?;
        let l1 = build_table(
            &filesystem,
            TableBuildRequest::new(
                &directory,
                &temporary,
                identity,
                11,
                &l1_entries,
                &limits,
                options,
            ),
        )?;
        let state = ManifestStateV2::initial(identity, 2, 1, 3)?.applied(
            &VersionEditV2 {
                next_file_number: Some(12),
                added_files: vec![
                    LevelFile {
                        level: 0,
                        metadata: l0.clone(),
                    },
                    LevelFile {
                        level: 1,
                        metadata: l1.clone(),
                    },
                ],
                ..VersionEditV2::default()
            },
            &limits,
        )?;
        let version = VersionSet::open_v2(&directory, identity, &state, &limits, 64 * 1024 * 1024)?;
        assert_eq!(version.level_count(), 3);
        assert_eq!(version.metadata_levels()[0], vec![l0]);
        assert_eq!(version.metadata_levels()[1], vec![l1]);
        assert_eq!(
            version.get_at(b"n", Some(SequenceNumber::new(2)?), &limits, true,)?,
            TableSnapshotLookup::Value(b"l1".to_vec())
        );
        drop(version);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
