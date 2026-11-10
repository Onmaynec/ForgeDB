use std::collections::{BTreeMap, BTreeSet};

use forgedb_core::{DatabaseId, Error, InternalKeyComparator, Limits, Result, SequenceNumber};
use forgedb_sstable::TableMeta;

use crate::{INITIAL_MANIFEST_NUMBER, MINIMUM_READER_VERSION};

const MAX_PERSISTED_LEVEL: u32 = 63;

/// Persisted metadata одного SSTable уровня L1+.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LevelFileMeta {
    pub level: u32,
    pub table: TableMeta,
}

impl LevelFileMeta {
    pub fn new(level: u32, table: TableMeta, limits: &Limits) -> Result<Self> {
        if level == 0 || level > MAX_PERSISTED_LEVEL {
            return Err(Error::invalid_argument(
                "level",
                "persisted leveled file должен принадлежать L1..L63",
            ));
        }
        table.validate(limits)?;
        Ok(Self { level, table })
    }
}

/// Ссылка на удаляемый SSTable уровня L1+.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LevelFileRef {
    pub level: u32,
    pub file_number: u64,
}

impl LevelFileRef {
    pub fn new(level: u32, file_number: u64) -> Result<Self> {
        if level == 0 || level > MAX_PERSISTED_LEVEL {
            return Err(Error::invalid_argument(
                "level",
                "persisted leveled file должен принадлежать L1..L63",
            ));
        }
        if file_number == 0 {
            return Err(Error::invalid_argument(
                "file_number",
                "номер SSTable не может быть нулём",
            ));
        }
        Ok(Self { level, file_number })
    }
}

/// Одно атомарное изменение VersionSet.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VersionEdit {
    pub comparator: Option<String>,
    pub log_number: Option<u64>,
    pub previous_log_number: Option<Option<u64>>,
    pub next_file_number: Option<u64>,
    pub last_sequence: Option<Option<SequenceNumber>>,
    /// L0 additions. Сохраняется для совместимости с MANIFEST V1.
    pub added_files: Vec<TableMeta>,
    /// L0 deletions. Сохраняется для совместимости с MANIFEST V1.
    pub deleted_files: Vec<u64>,
    /// L1+ additions, доступные начиная с MANIFEST V2.
    pub added_level_files: Vec<LevelFileMeta>,
    /// L1+ deletions, доступные начиная с MANIFEST V2.
    pub deleted_level_files: Vec<LevelFileRef>,
    pub minimum_reader_version: Option<u32>,
    pub feature_flags: Option<u64>,
}

impl VersionEdit {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.comparator.is_none()
            && self.log_number.is_none()
            && self.previous_log_number.is_none()
            && self.next_file_number.is_none()
            && self.last_sequence.is_none()
            && self.added_files.is_empty()
            && self.deleted_files.is_empty()
            && self.added_level_files.is_empty()
            && self.deleted_level_files.is_empty()
            && self.minimum_reader_version.is_none()
            && self.feature_flags.is_none()
    }
}

/// Полностью восстановленное authoritative состояние LSM.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestState {
    pub database_id: DatabaseId,
    pub manifest_number: u64,
    pub comparator: String,
    pub log_number: u64,
    pub previous_log_number: Option<u64>,
    pub next_file_number: u64,
    pub last_sequence: Option<SequenceNumber>,
    pub l0_files: Vec<TableMeta>,
    /// Flat metadata L1+, отсортированная по `(level, smallest_user_key, file_number)`.
    pub level_files: Vec<LevelFileMeta>,
    pub minimum_reader_version: u32,
    pub feature_flags: u64,
}

impl ManifestState {
    pub fn initial(database_id: DatabaseId, initial_log_number: u64) -> Result<Self> {
        if initial_log_number == 0 {
            return Err(Error::invalid_argument(
                "initial_log_number",
                "номер WAL не может быть нулём",
            ));
        }
        Ok(Self {
            database_id,
            manifest_number: INITIAL_MANIFEST_NUMBER,
            comparator: InternalKeyComparator::NAME.to_owned(),
            log_number: initial_log_number,
            previous_log_number: None,
            next_file_number: initial_log_number
                .checked_add(1)
                .ok_or(Error::ResourceLimit {
                    resource: "file_number",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?,
            last_sequence: None,
            l0_files: Vec::new(),
            level_files: Vec::new(),
            minimum_reader_version: MINIMUM_READER_VERSION,
            feature_flags: 0,
        })
    }

    #[must_use]
    pub fn snapshot_edit(&self) -> VersionEdit {
        VersionEdit {
            comparator: Some(self.comparator.clone()),
            log_number: Some(self.log_number),
            previous_log_number: Some(self.previous_log_number),
            next_file_number: Some(self.next_file_number),
            last_sequence: Some(self.last_sequence),
            added_files: self.l0_files.clone(),
            deleted_files: Vec::new(),
            added_level_files: self.level_files.clone(),
            deleted_level_files: Vec::new(),
            minimum_reader_version: Some(self.minimum_reader_version),
            feature_flags: Some(self.feature_flags),
        }
    }

    pub fn validate(&self, limits: &Limits) -> Result<()> {
        if self.manifest_number == 0 || self.log_number == 0 || self.next_file_number == 0 {
            return Err(Error::corruption(
                "MANIFEST state",
                "manifest/log/next file number не могут быть нулевыми",
            ));
        }
        if self.comparator != InternalKeyComparator::NAME {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST comparator",
                found: 0,
                minimum_supported: 1,
                maximum_supported: 1,
            });
        }
        if self.minimum_reader_version > MINIMUM_READER_VERSION {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST minimum reader version",
                found: self.minimum_reader_version,
                minimum_supported: MINIMUM_READER_VERSION,
                maximum_supported: MINIMUM_READER_VERSION,
            });
        }
        if self.feature_flags != 0 {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST feature flags",
                found: u32::try_from(self.feature_flags).unwrap_or(u32::MAX),
                minimum_supported: 0,
                maximum_supported: 0,
            });
        }
        if self.previous_log_number == Some(self.log_number) {
            return Err(Error::corruption(
                "MANIFEST state",
                "current и previous WAL совпадают",
            ));
        }

        let mut maximum_used = self.log_number.max(self.manifest_number);
        if let Some(previous) = self.previous_log_number {
            if previous == 0 {
                return Err(Error::corruption(
                    "MANIFEST state",
                    "previous_log_number равен нулю",
                ));
            }
            maximum_used = maximum_used.max(previous);
        }

        let mut seen = BTreeSet::new();
        for file in &self.l0_files {
            file.validate(limits)?;
            if !seen.insert(file.file_number) {
                return Err(Error::corruption(
                    "MANIFEST state",
                    "duplicate SSTable file_number",
                ));
            }
            maximum_used = maximum_used.max(file.file_number);
        }
        if self
            .l0_files
            .windows(2)
            .any(|pair| pair[0].file_number >= pair[1].file_number)
        {
            return Err(Error::corruption(
                "MANIFEST state",
                "L0 files не отсортированы по file_number",
            ));
        }

        let mut by_level = BTreeMap::<u32, Vec<&TableMeta>>::new();
        for leveled in &self.level_files {
            if leveled.level == 0 || leveled.level > MAX_PERSISTED_LEVEL {
                return Err(Error::corruption(
                    "MANIFEST state",
                    "L1+ metadata содержит недопустимый level",
                ));
            }
            leveled.table.validate(limits)?;
            if !seen.insert(leveled.table.file_number) {
                return Err(Error::corruption(
                    "MANIFEST state",
                    "duplicate SSTable file_number между уровнями",
                ));
            }
            maximum_used = maximum_used.max(leveled.table.file_number);
            by_level
                .entry(leveled.level)
                .or_default()
                .push(&leveled.table);
        }
        for files in by_level.values() {
            for pair in files.windows(2) {
                let left = pair[0];
                let right = pair[1];
                if left.smallest_user_key >= right.smallest_user_key {
                    return Err(Error::corruption(
                        "MANIFEST L1+",
                        "файлы уровня не отсортированы по smallest key",
                    ));
                }
                if left.largest_user_key >= right.smallest_user_key {
                    return Err(Error::corruption(
                        "MANIFEST L1+",
                        "key ranges внутри L1+ перекрываются",
                    ));
                }
            }
        }

        if self.next_file_number <= maximum_used {
            return Err(Error::corruption(
                "MANIFEST state",
                "next_file_number не больше уже использованных номеров",
            ));
        }
        Ok(())
    }

    /// Preflight-применяет edit к clone состояния.
    pub fn applied(&self, edit: &VersionEdit, limits: &Limits) -> Result<Self> {
        if edit.is_empty() {
            return Err(Error::invalid_argument(
                "version_edit",
                "пустой VersionEdit не записывается",
            ));
        }
        let mut next = self.clone();
        if let Some(comparator) = &edit.comparator {
            if comparator != InternalKeyComparator::NAME {
                return Err(Error::UnsupportedFormat {
                    component: "MANIFEST comparator",
                    found: 0,
                    minimum_supported: 1,
                    maximum_supported: 1,
                });
            }
            next.comparator = comparator.clone();
        }
        if let Some(log_number) = edit.log_number {
            if log_number == 0 {
                return Err(Error::corruption("VersionEdit", "log_number равен нулю"));
            }
            next.log_number = log_number;
        }
        if let Some(previous) = edit.previous_log_number {
            if previous == Some(0) {
                return Err(Error::corruption(
                    "VersionEdit",
                    "previous_log_number равен нулю",
                ));
            }
            next.previous_log_number = previous;
        }
        if let Some(number) = edit.next_file_number {
            if number == 0 {
                return Err(Error::corruption(
                    "VersionEdit",
                    "next_file_number равен нулю",
                ));
            }
            next.next_file_number = number;
        }
        if let Some(candidate) = edit.last_sequence {
            if let (Some(previous), Some(value)) = (next.last_sequence, candidate)
                && value < previous
            {
                return Err(Error::corruption("VersionEdit", "last_sequence уменьшился"));
            }
            next.last_sequence = candidate;
        }
        if let Some(minimum) = edit.minimum_reader_version {
            next.minimum_reader_version = minimum;
        }
        if let Some(flags) = edit.feature_flags {
            next.feature_flags = flags;
        }

        for number in &edit.deleted_files {
            let before = next.l0_files.len();
            next.l0_files.retain(|file| file.file_number != *number);
            if next.l0_files.len() == before {
                return Err(Error::corruption(
                    "VersionEdit",
                    "удаляемый L0 SSTable отсутствует",
                ));
            }
        }
        for deletion in &edit.deleted_level_files {
            if deletion.level == 0 || deletion.level > MAX_PERSISTED_LEVEL {
                return Err(Error::corruption(
                    "VersionEdit",
                    "удаляемый leveled SSTable содержит недопустимый level",
                ));
            }
            let before = next.level_files.len();
            next.level_files.retain(|file| {
                file.level != deletion.level || file.table.file_number != deletion.file_number
            });
            if next.level_files.len() == before {
                return Err(Error::corruption(
                    "VersionEdit",
                    "удаляемый leveled SSTable отсутствует",
                ));
            }
        }

        for file in &edit.added_files {
            file.validate(limits)?;
            if next
                .l0_files
                .iter()
                .any(|current| current.file_number == file.file_number)
                || next
                    .level_files
                    .iter()
                    .any(|current| current.table.file_number == file.file_number)
            {
                return Err(Error::corruption(
                    "VersionEdit",
                    "добавляемый SSTable уже существует",
                ));
            }
            next.l0_files.push(file.clone());
        }
        for leveled in &edit.added_level_files {
            if leveled.level == 0 || leveled.level > MAX_PERSISTED_LEVEL {
                return Err(Error::corruption(
                    "VersionEdit",
                    "добавляемый leveled SSTable содержит недопустимый level",
                ));
            }
            leveled.table.validate(limits)?;
            if next
                .l0_files
                .iter()
                .any(|current| current.file_number == leveled.table.file_number)
                || next
                    .level_files
                    .iter()
                    .any(|current| current.table.file_number == leveled.table.file_number)
            {
                return Err(Error::corruption(
                    "VersionEdit",
                    "добавляемый SSTable уже существует",
                ));
            }
            next.level_files.push(leveled.clone());
        }

        next.l0_files.sort_by_key(|file| file.file_number);
        next.level_files.sort_by(|left, right| {
            left.level
                .cmp(&right.level)
                .then_with(|| {
                    left.table
                        .smallest_user_key
                        .cmp(&right.table.smallest_user_key)
                })
                .then_with(|| left.table.file_number.cmp(&right.table.file_number))
        });
        next.validate(limits)?;
        Ok(next)
    }
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
    fn leveled_files_require_non_overlapping_sorted_ranges() -> Result<()> {
        let limits = Limits::default();
        let mut state = ManifestState::initial(DatabaseId::random(), 1)?;
        state.next_file_number = 13;
        state.level_files = vec![
            LevelFileMeta::new(1, metadata(10, b"a", b"m")?, &limits)?,
            LevelFileMeta::new(1, metadata(11, b"n", b"z")?, &limits)?,
        ];
        state.validate(&limits)?;
        state.level_files[1].table.smallest_user_key = b"m".to_vec();
        assert!(state.validate(&limits).is_err());
        Ok(())
    }

    #[test]
    fn compaction_edit_moves_file_from_l0_to_l1_atomically() -> Result<()> {
        let limits = Limits::default();
        let mut state = ManifestState::initial(DatabaseId::random(), 1)?;
        state.next_file_number = 20;
        state.l0_files.push(metadata(10, b"a", b"z")?);
        let edit = VersionEdit {
            deleted_files: vec![10],
            added_level_files: vec![LevelFileMeta::new(1, metadata(19, b"a", b"z")?, &limits)?],
            next_file_number: Some(20),
            ..VersionEdit::default()
        };
        let next = state.applied(&edit, &limits)?;
        assert!(next.l0_files.is_empty());
        assert_eq!(next.level_files.len(), 1);
        assert_eq!(next.level_files[0].level, 1);
        Ok(())
    }
}
