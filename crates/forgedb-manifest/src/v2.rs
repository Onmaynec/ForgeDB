use std::collections::BTreeSet;

use forgedb_core::{
    DatabaseId, Error, InternalKeyComparator, Limits, Result, SequenceNumber,
};
use forgedb_sstable::TableMeta;

use crate::checksum::crc32c;
use crate::{MANIFEST_FRAME_HEADER_SIZE, MANIFEST_HEADER_SIZE};

pub const MANIFEST_V2_FORMAT_VERSION: u16 = 2;
pub const MANIFEST_V2_MINIMUM_READER_VERSION: u32 = 2;
pub const MANIFEST_V2_MAX_LEVELS: usize = 64;

const MANIFEST_MAGIC: &[u8; 8] = b"FORGEMAN";
const FRAME_MAGIC: &[u8; 4] = b"FMV2";
const RECORD_VERSION_EDIT: u8 = 1;
const EDIT_VERSION: u8 = 2;
const TAG_COMPARATOR: u8 = 1;
const TAG_LOG_NUMBER: u8 = 2;
const TAG_PREVIOUS_LOG_NUMBER: u8 = 3;
const TAG_NEXT_FILE_NUMBER: u8 = 4;
const TAG_LAST_SEQUENCE: u8 = 5;
const TAG_ADD_FILE: u8 = 6;
const TAG_DELETE_FILE: u8 = 7;
const TAG_MINIMUM_READER_VERSION: u8 = 8;
const TAG_FEATURE_FLAGS: u8 = 9;
const TAG_LEVEL_COUNT: u8 = 10;
const TAG_COMPACTION_POINTER: u8 = 11;
const MAX_FIELDS: usize = 1_000_000;
const MAX_COMPARATOR_BYTES: usize = 256;
const TABLE_META_FIXED_SIZE: usize = 52;
const DELETED_FILE_SIZE: usize = 16;
const COMPACTION_POINTER_PREFIX_SIZE: usize = 8;

/// Заголовок MANIFEST V2 без привязки к filesystem implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManifestV2Header {
    pub database_id: DatabaseId,
    pub manifest_number: u64,
    pub created_unix_ms: u64,
}

impl ManifestV2Header {
    pub fn validate(self) -> Result<()> {
        if self.manifest_number == 0 {
            return Err(Error::invalid_argument(
                "manifest_number",
                "номер MANIFEST не может быть нулём",
            ));
        }
        Ok(())
    }
}

/// SSTable и его authoritative level в MANIFEST V2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LevelFile {
    pub level: u32,
    pub metadata: TableMeta,
}

/// Удаляемый SSTable с обязательным source level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeletedLevelFile {
    pub level: u32,
    pub file_number: u64,
}

/// Последняя обработанная user-key граница одного source level.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionPointer {
    pub level: u32,
    pub user_key: Vec<u8>,
}

/// Одно атомарное level-aware изменение MANIFEST V2.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VersionEditV2 {
    pub comparator: Option<String>,
    pub log_number: Option<u64>,
    pub previous_log_number: Option<Option<u64>>,
    pub next_file_number: Option<u64>,
    pub last_sequence: Option<Option<SequenceNumber>>,
    pub added_files: Vec<LevelFile>,
    pub deleted_files: Vec<DeletedLevelFile>,
    pub minimum_reader_version: Option<u32>,
    pub feature_flags: Option<u64>,
    pub level_count: Option<u32>,
    pub compaction_pointers: Vec<CompactionPointer>,
}

impl VersionEditV2 {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.comparator.is_none()
            && self.log_number.is_none()
            && self.previous_log_number.is_none()
            && self.next_file_number.is_none()
            && self.last_sequence.is_none()
            && self.added_files.is_empty()
            && self.deleted_files.is_empty()
            && self.minimum_reader_version.is_none()
            && self.feature_flags.is_none()
            && self.level_count.is_none()
            && self.compaction_pointers.is_empty()
    }
}

/// Полностью replayed authoritative level state MANIFEST V2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestStateV2 {
    pub database_id: DatabaseId,
    pub manifest_number: u64,
    pub comparator: String,
    pub log_number: u64,
    pub previous_log_number: Option<u64>,
    pub next_file_number: u64,
    pub last_sequence: Option<SequenceNumber>,
    pub levels: Vec<Vec<TableMeta>>,
    pub compaction_pointers: Vec<Option<Vec<u8>>>,
    pub minimum_reader_version: u32,
    pub feature_flags: u64,
}

impl ManifestStateV2 {
    pub fn initial(
        database_id: DatabaseId,
        manifest_number: u64,
        initial_log_number: u64,
        level_count: usize,
    ) -> Result<Self> {
        if manifest_number == 0 {
            return Err(Error::invalid_argument(
                "manifest_number",
                "номер MANIFEST не может быть нулём",
            ));
        }
        if initial_log_number == 0 {
            return Err(Error::invalid_argument(
                "initial_log_number",
                "номер WAL не может быть нулём",
            ));
        }
        validate_level_count(level_count)?;
        let next_file_number = initial_log_number
            .checked_add(1)
            .ok_or(Error::ResourceLimit {
                resource: "file_number",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        Ok(Self {
            database_id,
            manifest_number,
            comparator: InternalKeyComparator::NAME.to_owned(),
            log_number: initial_log_number,
            previous_log_number: None,
            next_file_number,
            last_sequence: None,
            levels: (0..level_count).map(|_| Vec::new()).collect(),
            compaction_pointers: (0..level_count).map(|_| None).collect(),
            minimum_reader_version: MANIFEST_V2_MINIMUM_READER_VERSION,
            feature_flags: 0,
        })
    }

    #[must_use]
    pub fn snapshot_edit(&self) -> VersionEditV2 {
        let added_files = self
            .levels
            .iter()
            .enumerate()
            .flat_map(|(level, files)| {
                files.iter().cloned().map(move |metadata| LevelFile {
                    level: u32::try_from(level).unwrap_or(u32::MAX),
                    metadata,
                })
            })
            .collect();
        let compaction_pointers = self
            .compaction_pointers
            .iter()
            .enumerate()
            .filter_map(|(level, key)| {
                key.as_ref().map(|user_key| CompactionPointer {
                    level: u32::try_from(level).unwrap_or(u32::MAX),
                    user_key: user_key.clone(),
                })
            })
            .collect();
        VersionEditV2 {
            comparator: Some(self.comparator.clone()),
            log_number: Some(self.log_number),
            previous_log_number: Some(self.previous_log_number),
            next_file_number: Some(self.next_file_number),
            last_sequence: Some(self.last_sequence),
            added_files,
            deleted_files: Vec::new(),
            minimum_reader_version: Some(self.minimum_reader_version),
            feature_flags: Some(self.feature_flags),
            level_count: Some(u32::try_from(self.levels.len()).unwrap_or(u32::MAX)),
            compaction_pointers,
        }
    }

    pub fn validate(&self, limits: &Limits) -> Result<()> {
        limits.validate()?;
        validate_level_count(self.levels.len())?;
        if self.compaction_pointers.len() != self.levels.len() {
            return Err(Error::corruption(
                "MANIFEST V2 state",
                "число compaction pointers не совпадает с числом levels",
            ));
        }
        if self.manifest_number == 0 || self.log_number == 0 || self.next_file_number == 0 {
            return Err(Error::corruption(
                "MANIFEST V2 state",
                "manifest/log/next file number не могут быть нулевыми",
            ));
        }
        if self.comparator != InternalKeyComparator::NAME {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST V2 comparator",
                found: 0,
                minimum_supported: 2,
                maximum_supported: 2,
            });
        }
        if self.minimum_reader_version != MANIFEST_V2_MINIMUM_READER_VERSION {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST V2 minimum reader version",
                found: self.minimum_reader_version,
                minimum_supported: MANIFEST_V2_MINIMUM_READER_VERSION,
                maximum_supported: MANIFEST_V2_MINIMUM_READER_VERSION,
            });
        }
        if self.feature_flags != 0 {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST V2 feature flags",
                found: u32::try_from(self.feature_flags).unwrap_or(u32::MAX),
                minimum_supported: 0,
                maximum_supported: 0,
            });
        }
        if self.previous_log_number == Some(self.log_number) {
            return Err(Error::corruption(
                "MANIFEST V2 state",
                "current и previous WAL совпадают",
            ));
        }

        let mut maximum_used = self.log_number.max(self.manifest_number);
        if let Some(previous) = self.previous_log_number {
            if previous == 0 {
                return Err(Error::corruption(
                    "MANIFEST V2 state",
                    "previous_log_number равен нулю",
                ));
            }
            maximum_used = maximum_used.max(previous);
        }
        let mut file_numbers = BTreeSet::new();
        for (level_index, files) in self.levels.iter().enumerate() {
            validate_level(level_index, files, limits)?;
            for file in files {
                if !file_numbers.insert(file.file_number) {
                    return Err(Error::corruption(
                        "MANIFEST V2 state",
                        "duplicate SSTable file number между levels",
                    ));
                }
                maximum_used = maximum_used.max(file.file_number);
            }
        }
        for pointer in self.compaction_pointers.iter().flatten() {
            limits.validate_key(pointer)?;
        }
        if self.next_file_number <= maximum_used {
            return Err(Error::corruption(
                "MANIFEST V2 state",
                "next_file_number не больше уже использованных номеров",
            ));
        }
        Ok(())
    }

    pub fn applied(&self, edit: &VersionEditV2, limits: &Limits) -> Result<Self> {
        if edit.is_empty() {
            return Err(Error::invalid_argument(
                "version_edit_v2",
                "пустой VersionEdit V2 не применяется",
            ));
        }
        let mut next = self.clone();
        if let Some(level_count) = edit.level_count {
            let level_count = usize::try_from(level_count).map_err(|_| Error::ResourceLimit {
                resource: "manifest_level_count",
                limit: u64::try_from(MANIFEST_V2_MAX_LEVELS).unwrap_or(u64::MAX),
                observed: u64::from(level_count),
            })?;
            validate_level_count(level_count)?;
            if level_count != next.levels.len() {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "level_count нельзя менять после создания MANIFEST",
                ));
            }
        }
        if let Some(comparator) = &edit.comparator {
            if comparator != InternalKeyComparator::NAME {
                return Err(Error::UnsupportedFormat {
                    component: "MANIFEST V2 comparator",
                    found: 0,
                    minimum_supported: 2,
                    maximum_supported: 2,
                });
            }
            next.comparator = comparator.clone();
        }
        if let Some(log_number) = edit.log_number {
            if log_number == 0 {
                return Err(Error::corruption("VersionEdit V2", "log_number равен нулю"));
            }
            next.log_number = log_number;
        }
        if let Some(previous) = edit.previous_log_number {
            if previous == Some(0) {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "previous_log_number равен нулю",
                ));
            }
            next.previous_log_number = previous;
        }
        if let Some(number) = edit.next_file_number {
            if number == 0 || number < next.next_file_number {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "next_file_number равен нулю или уменьшился",
                ));
            }
            next.next_file_number = number;
        }
        if let Some(candidate) = edit.last_sequence {
            if let (Some(previous), Some(value)) = (next.last_sequence, candidate)
                && value < previous
            {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "last_sequence уменьшился",
                ));
            }
            next.last_sequence = candidate;
        }
        if let Some(minimum) = edit.minimum_reader_version {
            next.minimum_reader_version = minimum;
        }
        if let Some(flags) = edit.feature_flags {
            next.feature_flags = flags;
        }

        for deleted in &edit.deleted_files {
            let level = level_index(deleted.level, next.levels.len())?;
            if deleted.file_number == 0 {
                return Err(Error::corruption(
                    "VersionEdit V2 delete file",
                    "file_number равен нулю",
                ));
            }
            let files = next.levels.get_mut(level).ok_or(Error::InternalInvariant {
                invariant: "I-MANIFEST-V2-DELETE-LEVEL",
                reason: "validated level отсутствует".to_owned(),
            })?;
            let before = files.len();
            files.retain(|file| file.file_number != deleted.file_number);
            if files.len() == before {
                return Err(Error::corruption(
                    "VersionEdit V2 delete file",
                    "удаляемый SSTable отсутствует в указанном level",
                ));
            }
        }

        for added in &edit.added_files {
            added.metadata.validate(limits)?;
            let level = level_index(added.level, next.levels.len())?;
            if next
                .levels
                .iter()
                .flatten()
                .any(|file| file.file_number == added.metadata.file_number)
            {
                return Err(Error::corruption(
                    "VersionEdit V2 add file",
                    "добавляемый file number уже существует",
                ));
            }
            next.levels
                .get_mut(level)
                .ok_or(Error::InternalInvariant {
                    invariant: "I-MANIFEST-V2-ADD-LEVEL",
                    reason: "validated level отсутствует".to_owned(),
                })?
                .push(added.metadata.clone());
        }

        for pointer in &edit.compaction_pointers {
            limits.validate_key(&pointer.user_key)?;
            let level = level_index(pointer.level, next.levels.len())?;
            let target = next
                .compaction_pointers
                .get_mut(level)
                .ok_or(Error::InternalInvariant {
                    invariant: "I-MANIFEST-V2-COMPACTION-POINTER",
                    reason: "validated pointer level отсутствует".to_owned(),
                })?;
            *target = Some(pointer.user_key.clone());
        }

        sort_levels(&mut next.levels);
        next.validate(limits)?;
        Ok(next)
    }
}

/// Bounded encoder/decoder persisted contract MANIFEST V2.
#[derive(Clone, Copy, Debug, Default)]
pub struct ManifestV2Codec;

impl ManifestV2Codec {
    pub fn encode_header(header: ManifestV2Header) -> Result<[u8; MANIFEST_HEADER_SIZE]> {
        header.validate()?;
        let mut output = [0_u8; MANIFEST_HEADER_SIZE];
        output[0..8].copy_from_slice(MANIFEST_MAGIC);
        output[8..10].copy_from_slice(&MANIFEST_V2_FORMAT_VERSION.to_le_bytes());
        output[10..12].copy_from_slice(&(MANIFEST_HEADER_SIZE as u16).to_le_bytes());
        output[12..16].copy_from_slice(&0_u32.to_le_bytes());
        output[16..32].copy_from_slice(header.database_id.as_bytes());
        output[32..40].copy_from_slice(&header.manifest_number.to_le_bytes());
        output[40..48].copy_from_slice(&header.created_unix_ms.to_le_bytes());
        output[48..52].copy_from_slice(&MANIFEST_V2_MINIMUM_READER_VERSION.to_le_bytes());
        output[52..60].copy_from_slice(&[0_u8; 8]);
        let checksum = crc32c(&output[..60]);
        output[60..64].copy_from_slice(&checksum.to_le_bytes());
        Ok(output)
    }

    pub fn decode_header(input: &[u8; MANIFEST_HEADER_SIZE]) -> Result<ManifestV2Header> {
        if &input[0..8] != MANIFEST_MAGIC {
            return Err(Error::corruption(
                "MANIFEST V2 header",
                "неверная сигнатура",
            ));
        }
        let version = read_u16(input, 8, "MANIFEST V2 header")?;
        if version != MANIFEST_V2_FORMAT_VERSION {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST V2",
                found: u32::from(version),
                minimum_supported: u32::from(MANIFEST_V2_FORMAT_VERSION),
                maximum_supported: u32::from(MANIFEST_V2_FORMAT_VERSION),
            });
        }
        if usize::from(read_u16(input, 10, "MANIFEST V2 header")?) != MANIFEST_HEADER_SIZE
            || read_u32(input, 12, "MANIFEST V2 header")? != 0
            || input[52..60].iter().any(|byte| *byte != 0)
        {
            return Err(Error::corruption(
                "MANIFEST V2 header",
                "header length или reserved fields неверны",
            ));
        }
        if crc32c(&input[..60]) != read_u32(input, 60, "MANIFEST V2 header")? {
            return Err(Error::corruption(
                "MANIFEST V2 header",
                "CRC32C не совпадает",
            ));
        }
        let minimum = read_u32(input, 48, "MANIFEST V2 header")?;
        if minimum != MANIFEST_V2_MINIMUM_READER_VERSION {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST V2 minimum reader version",
                found: minimum,
                minimum_supported: MANIFEST_V2_MINIMUM_READER_VERSION,
                maximum_supported: MANIFEST_V2_MINIMUM_READER_VERSION,
            });
        }
        let mut id = [0_u8; 16];
        id.copy_from_slice(&input[16..32]);
        let header = ManifestV2Header {
            database_id: DatabaseId::from_bytes(id),
            manifest_number: read_u64(input, 32, "MANIFEST V2 header")?,
            created_unix_ms: read_u64(input, 40, "MANIFEST V2 header")?,
        };
        header.validate()?;
        Ok(header)
    }

    pub fn encode_edit_frame(edit: &VersionEditV2, limits: &Limits) -> Result<Vec<u8>> {
        if edit.is_empty() {
            return Err(Error::invalid_argument(
                "version_edit_v2",
                "пустой VersionEdit V2 не кодируется",
            ));
        }
        let fields = encode_fields(edit, limits)?;
        if fields.len() > MAX_FIELDS {
            return Err(Error::ResourceLimit {
                resource: "manifest_v2_field_count",
                limit: u64::try_from(MAX_FIELDS).unwrap_or(u64::MAX),
                observed: u64::try_from(fields.len()).unwrap_or(u64::MAX),
            });
        }
        let mut payload = Vec::new();
        payload.push(EDIT_VERSION);
        payload.extend_from_slice(&[0_u8; 3]);
        for (tag, value) in &fields {
            payload.push(*tag);
            payload.extend_from_slice(&[0_u8; 3]);
            let length = u32::try_from(value.len()).map_err(|_| Error::ResourceLimit {
                resource: "manifest_v2_field_length",
                limit: u64::from(u32::MAX),
                observed: u64::try_from(value.len()).unwrap_or(u64::MAX),
            })?;
            payload.extend_from_slice(&length.to_le_bytes());
            payload.extend_from_slice(value);
        }
        if payload.len() > limits.max_batch_size {
            return Err(Error::ResourceLimit {
                resource: "manifest_v2_payload_length",
                limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
                observed: u64::try_from(payload.len()).unwrap_or(u64::MAX),
            });
        }
        let total = MANIFEST_FRAME_HEADER_SIZE
            .checked_add(payload.len())
            .ok_or(Error::ResourceLimit {
                resource: "manifest_v2_frame_length",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        let total_u32 = u32::try_from(total).map_err(|_| Error::ResourceLimit {
            resource: "manifest_v2_frame_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(total).unwrap_or(u64::MAX),
        })?;
        let payload_u32 = u32::try_from(payload.len()).map_err(|_| Error::ResourceLimit {
            resource: "manifest_v2_payload_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(payload.len()).unwrap_or(u64::MAX),
        })?;
        let field_count = u32::try_from(fields.len()).map_err(|_| Error::ResourceLimit {
            resource: "manifest_v2_field_count",
            limit: u64::from(u32::MAX),
            observed: u64::MAX,
        })?;
        let mut frame = vec![0_u8; MANIFEST_FRAME_HEADER_SIZE];
        frame[0..4].copy_from_slice(FRAME_MAGIC);
        frame[4..8].copy_from_slice(&total_u32.to_le_bytes());
        frame[12] = RECORD_VERSION_EDIT;
        frame[13] = 0;
        frame[14..16].copy_from_slice(&MANIFEST_V2_FORMAT_VERSION.to_le_bytes());
        frame[16..20].copy_from_slice(&field_count.to_le_bytes());
        frame[20..24].copy_from_slice(&payload_u32.to_le_bytes());
        frame[24..32].copy_from_slice(&0_u64.to_le_bytes());
        frame.extend_from_slice(&payload);
        let checksum = crc32c(&frame[12..]);
        frame[8..12].copy_from_slice(&checksum.to_le_bytes());
        Ok(frame)
    }

    pub fn decode_edit_frame(frame: &[u8], limits: &Limits) -> Result<VersionEditV2> {
        if frame.len() < MANIFEST_FRAME_HEADER_SIZE {
            return Err(Error::corruption(
                "MANIFEST V2 frame",
                "frame header усечён",
            ));
        }
        if frame.get(0..4) != Some(FRAME_MAGIC.as_slice()) {
            return Err(Error::corruption(
                "MANIFEST V2 frame",
                "неверная сигнатура",
            ));
        }
        let version = read_u16(frame, 14, "MANIFEST V2 frame")?;
        if version != MANIFEST_V2_FORMAT_VERSION {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST V2 frame",
                found: u32::from(version),
                minimum_supported: u32::from(MANIFEST_V2_FORMAT_VERSION),
                maximum_supported: u32::from(MANIFEST_V2_FORMAT_VERSION),
            });
        }
        if frame[12] != RECORD_VERSION_EDIT || frame[13] != 0 {
            return Err(Error::UnsupportedFormat {
                component: "MANIFEST V2 record type",
                found: u32::from(frame[12]),
                minimum_supported: u32::from(RECORD_VERSION_EDIT),
                maximum_supported: u32::from(RECORD_VERSION_EDIT),
            });
        }
        if read_u64(frame, 24, "MANIFEST V2 frame")? != 0 {
            return Err(Error::corruption(
                "MANIFEST V2 frame",
                "reserved bytes должны быть нулевыми",
            ));
        }
        let total = usize::try_from(read_u32(frame, 4, "MANIFEST V2 frame")?).map_err(|_| {
            Error::ResourceLimit {
                resource: "manifest_v2_frame_length",
                limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
                observed: u64::MAX,
            }
        })?;
        let payload_length =
            usize::try_from(read_u32(frame, 20, "MANIFEST V2 frame")?).map_err(|_| {
                Error::ResourceLimit {
                    resource: "manifest_v2_payload_length",
                    limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
                    observed: u64::MAX,
                }
            })?;
        let expected = MANIFEST_FRAME_HEADER_SIZE
            .checked_add(payload_length)
            .ok_or_else(|| Error::corruption("MANIFEST V2 frame", "длина переполнена"))?;
        if total != expected || total != frame.len() || payload_length > limits.max_batch_size {
            return Err(Error::corruption(
                "MANIFEST V2 frame",
                "total/payload length не совпадает с физическим frame",
            ));
        }
        let field_count =
            usize::try_from(read_u32(frame, 16, "MANIFEST V2 frame")?).map_err(|_| {
                Error::ResourceLimit {
                    resource: "manifest_v2_field_count",
                    limit: u64::try_from(MAX_FIELDS).unwrap_or(u64::MAX),
                    observed: u64::MAX,
                }
            })?;
        if field_count == 0 || field_count > MAX_FIELDS {
            return Err(Error::ResourceLimit {
                resource: "manifest_v2_field_count",
                limit: u64::try_from(MAX_FIELDS).unwrap_or(u64::MAX),
                observed: u64::try_from(field_count).unwrap_or(u64::MAX),
            });
        }
        if crc32c(&frame[12..]) != read_u32(frame, 8, "MANIFEST V2 frame")? {
            return Err(Error::corruption(
                "MANIFEST V2 frame",
                "CRC32C не совпадает",
            ));
        }
        let payload = frame
            .get(MANIFEST_FRAME_HEADER_SIZE..)
            .ok_or_else(|| Error::corruption("MANIFEST V2 frame", "payload отсутствует"))?;
        decode_payload(payload, field_count, limits)
    }
}

fn encode_fields(edit: &VersionEditV2, limits: &Limits) -> Result<Vec<(u8, Vec<u8>)>> {
    let mut fields = Vec::new();
    if let Some(comparator) = &edit.comparator {
        if comparator.len() > MAX_COMPARATOR_BYTES {
            return Err(Error::ResourceLimit {
                resource: "manifest_v2_comparator_bytes",
                limit: u64::try_from(MAX_COMPARATOR_BYTES).unwrap_or(u64::MAX),
                observed: u64::try_from(comparator.len()).unwrap_or(u64::MAX),
            });
        }
        fields.push((TAG_COMPARATOR, comparator.as_bytes().to_vec()));
    }
    if let Some(number) = edit.log_number {
        fields.push((TAG_LOG_NUMBER, number.to_le_bytes().to_vec()));
    }
    if let Some(number) = edit.previous_log_number {
        fields.push((
            TAG_PREVIOUS_LOG_NUMBER,
            number.unwrap_or(0).to_le_bytes().to_vec(),
        ));
    }
    if let Some(number) = edit.next_file_number {
        fields.push((TAG_NEXT_FILE_NUMBER, number.to_le_bytes().to_vec()));
    }
    if let Some(sequence) = edit.last_sequence {
        fields.push((
            TAG_LAST_SEQUENCE,
            sequence
                .map_or(0, SequenceNumber::get)
                .to_le_bytes()
                .to_vec(),
        ));
    }
    for file in &edit.added_files {
        fields.push((TAG_ADD_FILE, encode_level_file(file, limits)?));
    }
    for file in &edit.deleted_files {
        fields.push((TAG_DELETE_FILE, encode_deleted_file(*file)?));
    }
    if let Some(version) = edit.minimum_reader_version {
        fields.push((TAG_MINIMUM_READER_VERSION, version.to_le_bytes().to_vec()));
    }
    if let Some(flags) = edit.feature_flags {
        fields.push((TAG_FEATURE_FLAGS, flags.to_le_bytes().to_vec()));
    }
    if let Some(level_count) = edit.level_count {
        fields.push((TAG_LEVEL_COUNT, level_count.to_le_bytes().to_vec()));
    }
    for pointer in &edit.compaction_pointers {
        fields.push((
            TAG_COMPACTION_POINTER,
            encode_compaction_pointer(pointer, limits)?,
        ));
    }
    Ok(fields)
}

fn decode_payload(payload: &[u8], field_count: usize, limits: &Limits) -> Result<VersionEditV2> {
    if payload.len() < 4 || payload[0] != EDIT_VERSION || payload[1..4] != [0_u8; 3] {
        return Err(Error::UnsupportedFormat {
            component: "VersionEdit V2",
            found: u32::from(payload.first().copied().unwrap_or(0)),
            minimum_supported: u32::from(EDIT_VERSION),
            maximum_supported: u32::from(EDIT_VERSION),
        });
    }
    let mut edit = VersionEditV2::default();
    let mut offset = 4_usize;
    for _ in 0..field_count {
        let field_header_end = offset
            .checked_add(8)
            .ok_or_else(|| Error::corruption("VersionEdit V2", "offset переполнен"))?;
        if field_header_end > payload.len() {
            return Err(Error::corruption(
                "VersionEdit V2",
                "field header усечён",
            ));
        }
        let tag = payload[offset];
        if payload[offset + 1..offset + 4] != [0_u8; 3] {
            return Err(Error::corruption(
                "VersionEdit V2",
                "reserved field bytes не равны нулю",
            ));
        }
        let length = usize::try_from(read_u32(payload, offset + 4, "VersionEdit V2")?)
            .map_err(|_| Error::ResourceLimit {
                resource: "manifest_v2_field_length",
                limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
                observed: u64::MAX,
            })?;
        let value_end = field_header_end
            .checked_add(length)
            .ok_or_else(|| Error::corruption("VersionEdit V2", "field range переполнен"))?;
        let value = payload
            .get(field_header_end..value_end)
            .ok_or_else(|| Error::corruption("VersionEdit V2", "field value усечено"))?;
        decode_field(&mut edit, tag, value, limits)?;
        offset = value_end;
    }
    if offset != payload.len() || edit.is_empty() {
        return Err(Error::corruption(
            "VersionEdit V2",
            "field count или trailing bytes неверны",
        ));
    }
    Ok(edit)
}

fn decode_field(edit: &mut VersionEditV2, tag: u8, value: &[u8], limits: &Limits) -> Result<()> {
    match tag {
        TAG_COMPARATOR => {
            if edit.comparator.is_some() || value.len() > MAX_COMPARATOR_BYTES {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "comparator повторён или oversized",
                ));
            }
            edit.comparator = Some(
                std::str::from_utf8(value)
                    .map_err(|_| Error::corruption("VersionEdit V2", "comparator не UTF-8"))?
                    .to_owned(),
            );
        }
        TAG_LOG_NUMBER => set_once_u64(&mut edit.log_number, value, "log_number")?,
        TAG_PREVIOUS_LOG_NUMBER => {
            if edit.previous_log_number.is_some() {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "previous_log_number повторяется",
                ));
            }
            let number = exact_u64(value, "previous_log_number")?;
            edit.previous_log_number = Some((number != 0).then_some(number));
        }
        TAG_NEXT_FILE_NUMBER => {
            set_once_u64(&mut edit.next_file_number, value, "next_file_number")?;
        }
        TAG_LAST_SEQUENCE => {
            if edit.last_sequence.is_some() {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "last_sequence повторяется",
                ));
            }
            let sequence = exact_u64(value, "last_sequence")?;
            edit.last_sequence = Some(if sequence == 0 {
                None
            } else {
                Some(SequenceNumber::new(sequence).map_err(|_| {
                    Error::corruption("VersionEdit V2", "last_sequence равен нулю")
                })?)
            });
        }
        TAG_ADD_FILE => edit.added_files.push(decode_level_file(value, limits)?),
        TAG_DELETE_FILE => edit.deleted_files.push(decode_deleted_file(value)?),
        TAG_MINIMUM_READER_VERSION => {
            if edit.minimum_reader_version.is_some() || value.len() != 4 {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "minimum_reader_version повторён или неверной ширины",
                ));
            }
            edit.minimum_reader_version = Some(read_u32(value, 0, "VersionEdit V2")?);
        }
        TAG_FEATURE_FLAGS => set_once_u64(&mut edit.feature_flags, value, "feature_flags")?,
        TAG_LEVEL_COUNT => {
            if edit.level_count.is_some() || value.len() != 4 {
                return Err(Error::corruption(
                    "VersionEdit V2",
                    "level_count повторён или неверной ширины",
                ));
            }
            edit.level_count = Some(read_u32(value, 0, "VersionEdit V2")?);
        }
        TAG_COMPACTION_POINTER => edit
            .compaction_pointers
            .push(decode_compaction_pointer(value, limits)?),
        _ => {
            return Err(Error::UnsupportedFormat {
                component: "VersionEdit V2 field tag",
                found: u32::from(tag),
                minimum_supported: u32::from(TAG_COMPARATOR),
                maximum_supported: u32::from(TAG_COMPACTION_POINTER),
            });
        }
    }
    Ok(())
}

fn encode_level_file(file: &LevelFile, limits: &Limits) -> Result<Vec<u8>> {
    file.metadata.validate(limits)?;
    if usize::try_from(file.level).map_or(true, |level| level >= MANIFEST_V2_MAX_LEVELS) {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_level",
            limit: u64::try_from(MANIFEST_V2_MAX_LEVELS - 1).unwrap_or(u64::MAX),
            observed: u64::from(file.level),
        });
    }
    let smallest = u32::try_from(file.metadata.smallest_user_key.len()).map_err(|_| {
        Error::ResourceLimit {
            resource: "manifest_v2_smallest_key_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(file.metadata.smallest_user_key.len()).unwrap_or(u64::MAX),
        }
    })?;
    let largest = u32::try_from(file.metadata.largest_user_key.len()).map_err(|_| {
        Error::ResourceLimit {
            resource: "manifest_v2_largest_key_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(file.metadata.largest_user_key.len()).unwrap_or(u64::MAX),
        }
    })?;
    let capacity = TABLE_META_FIXED_SIZE
        .checked_add(file.metadata.smallest_user_key.len())
        .and_then(|value| value.checked_add(file.metadata.largest_user_key.len()))
        .ok_or(Error::ResourceLimit {
            resource: "manifest_v2_table_meta_length",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    let mut output = Vec::with_capacity(capacity);
    output.extend_from_slice(&file.level.to_le_bytes());
    output.extend_from_slice(&file.metadata.file_number.to_le_bytes());
    output.extend_from_slice(&file.metadata.file_size.to_le_bytes());
    output.extend_from_slice(&smallest.to_le_bytes());
    output.extend_from_slice(&largest.to_le_bytes());
    output.extend_from_slice(&file.metadata.smallest_sequence.get().to_le_bytes());
    output.extend_from_slice(&file.metadata.largest_sequence.get().to_le_bytes());
    output.extend_from_slice(&file.metadata.entry_count.to_le_bytes());
    output.extend_from_slice(&file.metadata.smallest_user_key);
    output.extend_from_slice(&file.metadata.largest_user_key);
    Ok(output)
}

fn decode_level_file(value: &[u8], limits: &Limits) -> Result<LevelFile> {
    if value.len() < TABLE_META_FIXED_SIZE {
        return Err(Error::corruption(
            "VersionEdit V2 add file",
            "record усечён",
        ));
    }
    let level = read_u32(value, 0, "VersionEdit V2 add file")?;
    if usize::try_from(level).map_or(true, |index| index >= MANIFEST_V2_MAX_LEVELS) {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_level",
            limit: u64::try_from(MANIFEST_V2_MAX_LEVELS - 1).unwrap_or(u64::MAX),
            observed: u64::from(level),
        });
    }
    let smallest = usize::try_from(read_u32(value, 20, "VersionEdit V2 add file")?).map_err(
        |_| Error::ResourceLimit {
            resource: "manifest_v2_smallest_key_length",
            limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
            observed: u64::MAX,
        },
    )?;
    let largest = usize::try_from(read_u32(value, 24, "VersionEdit V2 add file")?).map_err(
        |_| Error::ResourceLimit {
            resource: "manifest_v2_largest_key_length",
            limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
            observed: u64::MAX,
        },
    )?;
    if smallest > limits.max_key_size || largest > limits.max_key_size {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_key_length",
            limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
            observed: u64::try_from(smallest.max(largest)).unwrap_or(u64::MAX),
        });
    }
    let smallest_end = TABLE_META_FIXED_SIZE
        .checked_add(smallest)
        .ok_or_else(|| Error::corruption("VersionEdit V2 add file", "key range переполнен"))?;
    let end = smallest_end
        .checked_add(largest)
        .ok_or_else(|| Error::corruption("VersionEdit V2 add file", "key range переполнен"))?;
    if end != value.len() {
        return Err(Error::corruption(
            "VersionEdit V2 add file",
            "record length не совпадает с key lengths",
        ));
    }
    let metadata = TableMeta {
        file_number: read_u64(value, 4, "VersionEdit V2 add file")?,
        file_size: read_u64(value, 12, "VersionEdit V2 add file")?,
        smallest_user_key: value[TABLE_META_FIXED_SIZE..smallest_end].to_vec(),
        largest_user_key: value[smallest_end..end].to_vec(),
        smallest_sequence: SequenceNumber::new(read_u64(
            value,
            28,
            "VersionEdit V2 add file",
        )?)
        .map_err(|_| {
            Error::corruption("VersionEdit V2 add file", "smallest_sequence равен нулю")
        })?,
        largest_sequence: SequenceNumber::new(read_u64(
            value,
            36,
            "VersionEdit V2 add file",
        )?)
        .map_err(|_| {
            Error::corruption("VersionEdit V2 add file", "largest_sequence равен нулю")
        })?,
        entry_count: read_u64(value, 44, "VersionEdit V2 add file")?,
    };
    metadata.validate(limits)?;
    Ok(LevelFile { level, metadata })
}

fn encode_deleted_file(file: DeletedLevelFile) -> Result<Vec<u8>> {
    if file.file_number == 0 {
        return Err(Error::invalid_argument(
            "deleted_file_number",
            "file number не может быть нулём",
        ));
    }
    if usize::try_from(file.level).map_or(true, |level| level >= MANIFEST_V2_MAX_LEVELS) {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_level",
            limit: u64::try_from(MANIFEST_V2_MAX_LEVELS - 1).unwrap_or(u64::MAX),
            observed: u64::from(file.level),
        });
    }
    let mut output = Vec::with_capacity(DELETED_FILE_SIZE);
    output.extend_from_slice(&file.level.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&file.file_number.to_le_bytes());
    Ok(output)
}

fn decode_deleted_file(value: &[u8]) -> Result<DeletedLevelFile> {
    if value.len() != DELETED_FILE_SIZE || read_u32(value, 4, "VersionEdit V2 delete file")? != 0 {
        return Err(Error::corruption(
            "VersionEdit V2 delete file",
            "record width или reserved bytes неверны",
        ));
    }
    let file = DeletedLevelFile {
        level: read_u32(value, 0, "VersionEdit V2 delete file")?,
        file_number: read_u64(value, 8, "VersionEdit V2 delete file")?,
    };
    if file.file_number == 0 {
        return Err(Error::corruption(
            "VersionEdit V2 delete file",
            "file number равен нулю",
        ));
    }
    Ok(file)
}

fn encode_compaction_pointer(pointer: &CompactionPointer, limits: &Limits) -> Result<Vec<u8>> {
    limits.validate_key(&pointer.user_key)?;
    if usize::try_from(pointer.level).map_or(true, |level| level >= MANIFEST_V2_MAX_LEVELS) {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_level",
            limit: u64::try_from(MANIFEST_V2_MAX_LEVELS - 1).unwrap_or(u64::MAX),
            observed: u64::from(pointer.level),
        });
    }
    let key_length = u32::try_from(pointer.user_key.len()).map_err(|_| Error::ResourceLimit {
        resource: "manifest_v2_compaction_pointer_length",
        limit: u64::from(u32::MAX),
        observed: u64::try_from(pointer.user_key.len()).unwrap_or(u64::MAX),
    })?;
    let capacity = COMPACTION_POINTER_PREFIX_SIZE
        .checked_add(pointer.user_key.len())
        .ok_or(Error::ResourceLimit {
            resource: "manifest_v2_compaction_pointer_length",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    let mut output = Vec::with_capacity(capacity);
    output.extend_from_slice(&pointer.level.to_le_bytes());
    output.extend_from_slice(&key_length.to_le_bytes());
    output.extend_from_slice(&pointer.user_key);
    Ok(output)
}

fn decode_compaction_pointer(value: &[u8], limits: &Limits) -> Result<CompactionPointer> {
    if value.len() < COMPACTION_POINTER_PREFIX_SIZE {
        return Err(Error::corruption(
            "VersionEdit V2 compaction pointer",
            "record усечён",
        ));
    }
    let level = read_u32(value, 0, "VersionEdit V2 compaction pointer")?;
    let key_length = usize::try_from(read_u32(
        value,
        4,
        "VersionEdit V2 compaction pointer",
    )?)
    .map_err(|_| Error::ResourceLimit {
        resource: "manifest_v2_compaction_pointer_length",
        limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
        observed: u64::MAX,
    })?;
    if key_length == 0 || key_length > limits.max_key_size {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_compaction_pointer_length",
            limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
            observed: u64::try_from(key_length).unwrap_or(u64::MAX),
        });
    }
    let expected = COMPACTION_POINTER_PREFIX_SIZE
        .checked_add(key_length)
        .ok_or_else(|| {
            Error::corruption(
                "VersionEdit V2 compaction pointer",
                "record length переполнена",
            )
        })?;
    if value.len() != expected {
        return Err(Error::corruption(
            "VersionEdit V2 compaction pointer",
            "record length не совпадает с key length",
        ));
    }
    let pointer = CompactionPointer {
        level,
        user_key: value[COMPACTION_POINTER_PREFIX_SIZE..].to_vec(),
    };
    limits.validate_key(&pointer.user_key)?;
    Ok(pointer)
}

fn validate_level_count(level_count: usize) -> Result<()> {
    if !(2..=MANIFEST_V2_MAX_LEVELS).contains(&level_count) {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_level_count",
            limit: u64::try_from(MANIFEST_V2_MAX_LEVELS).unwrap_or(u64::MAX),
            observed: u64::try_from(level_count).unwrap_or(u64::MAX),
        });
    }
    Ok(())
}

fn level_index(level: u32, level_count: usize) -> Result<usize> {
    let index = usize::try_from(level).map_err(|_| Error::ResourceLimit {
        resource: "manifest_v2_level",
        limit: u64::try_from(level_count.saturating_sub(1)).unwrap_or(u64::MAX),
        observed: u64::from(level),
    })?;
    if index >= level_count {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_level",
            limit: u64::try_from(level_count.saturating_sub(1)).unwrap_or(u64::MAX),
            observed: u64::from(level),
        });
    }
    Ok(index)
}

fn validate_level(level_index: usize, files: &[TableMeta], limits: &Limits) -> Result<()> {
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
                "MANIFEST V2 L0",
                "file numbers не строго возрастают",
            ));
        }
        return Ok(());
    }
    for pair in files.windows(2) {
        let left = pair.first().ok_or(Error::InternalInvariant {
            invariant: "I-MANIFEST-V2-LEVEL-LEFT",
            reason: "level pair не содержит left file".to_owned(),
        })?;
        let right = pair.get(1).ok_or(Error::InternalInvariant {
            invariant: "I-MANIFEST-V2-LEVEL-RIGHT",
            reason: "level pair не содержит right file".to_owned(),
        })?;
        if left.smallest_user_key >= right.smallest_user_key
            || left.largest_user_key >= right.smallest_user_key
        {
            return Err(Error::corruption(
                "MANIFEST V2 L1+",
                "files не отсортированы или key ranges перекрываются",
            ));
        }
    }
    Ok(())
}

fn sort_levels(levels: &mut [Vec<TableMeta>]) {
    for (level_index, files) in levels.iter_mut().enumerate() {
        if level_index == 0 {
            files.sort_by_key(|file| file.file_number);
        } else {
            files.sort_by(|left, right| left.smallest_user_key.cmp(&right.smallest_user_key));
        }
    }
}

fn set_once_u64(target: &mut Option<u64>, value: &[u8], name: &'static str) -> Result<()> {
    if target.is_some() {
        return Err(Error::corruption(
            "VersionEdit V2",
            format!("поле {name} повторяется"),
        ));
    }
    *target = Some(exact_u64(value, name)?);
    Ok(())
}

fn exact_u64(value: &[u8], name: &'static str) -> Result<u64> {
    if value.len() != 8 {
        return Err(Error::corruption(
            "VersionEdit V2",
            format!("поле {name} имеет неверную ширину"),
        ));
    }
    read_u64(value, 0, "VersionEdit V2")
}

fn read_u16(input: &[u8], offset: usize, component: &'static str) -> Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| Error::corruption(component, "u16 offset переполнен"))?;
    let bytes: [u8; 2] = input
        .get(offset..end)
        .ok_or_else(|| Error::corruption(component, "поле u16 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption(component, "поле u16 имеет неверную ширину"))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(input: &[u8], offset: usize, component: &'static str) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| Error::corruption(component, "u32 offset переполнен"))?;
    let bytes: [u8; 4] = input
        .get(offset..end)
        .ok_or_else(|| Error::corruption(component, "поле u32 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption(component, "поле u32 имеет неверную ширину"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &[u8], offset: usize, component: &'static str) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| Error::corruption(component, "u64 offset переполнен"))?;
    let bytes: [u8; 8] = input
        .get(offset..end)
        .ok_or_else(|| Error::corruption(component, "поле u64 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption(component, "поле u64 имеет неверную ширину"))?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(number: u64, smallest: &[u8], largest: &[u8]) -> TableMeta {
        TableMeta {
            file_number: number,
            file_size: 256,
            smallest_user_key: smallest.to_vec(),
            largest_user_key: largest.to_vec(),
            smallest_sequence: SequenceNumber::FIRST,
            largest_sequence: SequenceNumber::FIRST,
            entry_count: 1,
        }
    }

    fn edit() -> VersionEditV2 {
        VersionEditV2 {
            comparator: Some(InternalKeyComparator::NAME.to_owned()),
            log_number: Some(10),
            previous_log_number: Some(Some(9)),
            next_file_number: Some(40),
            last_sequence: Some(Some(SequenceNumber::FIRST)),
            added_files: vec![
                LevelFile {
                    level: 0,
                    metadata: metadata(20, b"a", b"c"),
                },
                LevelFile {
                    level: 1,
                    metadata: metadata(30, b"d", b"z"),
                },
            ],
            deleted_files: vec![DeletedLevelFile {
                level: 0,
                file_number: 19,
            }],
            minimum_reader_version: Some(MANIFEST_V2_MINIMUM_READER_VERSION),
            feature_flags: Some(0),
            level_count: Some(7),
            compaction_pointers: vec![CompactionPointer {
                level: 1,
                user_key: b"m".to_vec(),
            }],
        }
    }

    #[test]
    fn header_round_trip() -> Result<()> {
        let header = ManifestV2Header {
            database_id: DatabaseId::random(),
            manifest_number: 7,
            created_unix_ms: 42,
        };
        let encoded = ManifestV2Codec::encode_header(header)?;
        assert_eq!(ManifestV2Codec::decode_header(&encoded)?, header);
        Ok(())
    }

    #[test]
    fn edit_round_trip_preserves_levels_and_pointer() -> Result<()> {
        let edit = edit();
        let encoded = ManifestV2Codec::encode_edit_frame(&edit, &Limits::default())?;
        assert_eq!(
            ManifestV2Codec::decode_edit_frame(&encoded, &Limits::default())?,
            edit
        );
        Ok(())
    }

    #[test]
    fn future_header_and_frame_versions_are_rejected() -> Result<()> {
        let header = ManifestV2Header {
            database_id: DatabaseId::random(),
            manifest_number: 1,
            created_unix_ms: 0,
        };
        let mut encoded_header = ManifestV2Codec::encode_header(header)?;
        encoded_header[8..10].copy_from_slice(&3_u16.to_le_bytes());
        assert!(matches!(
            ManifestV2Codec::decode_header(&encoded_header),
            Err(Error::UnsupportedFormat { .. })
        ));

        let mut encoded_frame =
            ManifestV2Codec::encode_edit_frame(&edit(), &Limits::default())?;
        encoded_frame[14..16].copy_from_slice(&3_u16.to_le_bytes());
        assert!(matches!(
            ManifestV2Codec::decode_edit_frame(&encoded_frame, &Limits::default()),
            Err(Error::UnsupportedFormat { .. })
        ));
        Ok(())
    }

    #[test]
    fn state_applies_levelled_add_delete_and_pointer() -> Result<()> {
        let database_id = DatabaseId::random();
        let initial = ManifestStateV2::initial(database_id, 2, 10, 3)?;
        let seeded = initial.applied(
            &VersionEditV2 {
                next_file_number: Some(22),
                added_files: vec![
                    LevelFile {
                        level: 0,
                        metadata: metadata(20, b"a", b"c"),
                    },
                    LevelFile {
                        level: 1,
                        metadata: metadata(21, b"d", b"f"),
                    },
                ],
                ..VersionEditV2::default()
            },
            &Limits::default(),
        )?;
        let compacted = seeded.applied(
            &VersionEditV2 {
                next_file_number: Some(24),
                deleted_files: vec![DeletedLevelFile {
                    level: 0,
                    file_number: 20,
                }],
                added_files: vec![LevelFile {
                    level: 1,
                    metadata: metadata(23, b"a", b"c"),
                }],
                compaction_pointers: vec![CompactionPointer {
                    level: 0,
                    user_key: b"c".to_vec(),
                }],
                ..VersionEditV2::default()
            },
            &Limits::default(),
        )?;
        assert!(compacted.levels[0].is_empty());
        assert_eq!(compacted.levels[1].len(), 2);
        assert_eq!(compacted.compaction_pointers[0], Some(b"c".to_vec()));
        Ok(())
    }

    #[test]
    fn overlapping_l1_files_are_rejected() -> Result<()> {
        let state = ManifestStateV2::initial(DatabaseId::random(), 2, 10, 3)?;
        let result = state.applied(
            &VersionEditV2 {
                next_file_number: Some(22),
                added_files: vec![
                    LevelFile {
                        level: 1,
                        metadata: metadata(20, b"a", b"m"),
                    },
                    LevelFile {
                        level: 1,
                        metadata: metadata(21, b"m", b"z"),
                    },
                ],
                ..VersionEditV2::default()
            },
            &Limits::default(),
        );
        assert!(matches!(result, Err(Error::Corruption { .. })));
        Ok(())
    }

    #[test]
    fn delete_requires_exact_level() -> Result<()> {
        let state = ManifestStateV2::initial(DatabaseId::random(), 2, 10, 3)?.applied(
            &VersionEditV2 {
                next_file_number: Some(21),
                added_files: vec![LevelFile {
                    level: 1,
                    metadata: metadata(20, b"a", b"z"),
                }],
                ..VersionEditV2::default()
            },
            &Limits::default(),
        )?;
        assert!(matches!(
            state.applied(
                &VersionEditV2 {
                    deleted_files: vec![DeletedLevelFile {
                        level: 0,
                        file_number: 20,
                    }],
                    ..VersionEditV2::default()
                },
                &Limits::default()
            ),
            Err(Error::Corruption { .. })
        ));
        Ok(())
    }

    #[test]
    fn invalid_level_and_target_overflow_are_rejected() -> Result<()> {
        let mut invalid = edit();
        invalid.added_files[0].level = u32::MAX;
        assert!(ManifestV2Codec::encode_edit_frame(&invalid, &Limits::default()).is_err());
        assert!(ManifestStateV2::initial(
            DatabaseId::random(),
            1,
            1,
            MANIFEST_V2_MAX_LEVELS + 1
        )
        .is_err());
        Ok(())
    }
}
