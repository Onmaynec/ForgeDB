use forgedb_core::{Error, Limits, Result, SequenceNumber};
use forgedb_sstable::TableMeta;

use crate::checksum::crc32c;
use crate::state::{LevelFileMeta, LevelFileRef, VersionEdit};
use crate::{MANIFEST_FORMAT_VERSION, MANIFEST_FRAME_HEADER_SIZE, MINIMUM_MANIFEST_FORMAT_VERSION};

const FRAME_MAGIC_V1: &[u8; 4] = b"FMV1";
const FRAME_MAGIC_V2: &[u8; 4] = b"FMV2";
const RECORD_VERSION_EDIT: u8 = 1;
const TAG_COMPARATOR: u8 = 1;
const TAG_LOG_NUMBER: u8 = 2;
const TAG_PREVIOUS_LOG_NUMBER: u8 = 3;
const TAG_NEXT_FILE_NUMBER: u8 = 4;
const TAG_LAST_SEQUENCE: u8 = 5;
const TAG_ADD_FILE: u8 = 6;
const TAG_DELETE_FILE: u8 = 7;
const TAG_MINIMUM_READER_VERSION: u8 = 8;
const TAG_FEATURE_FLAGS: u8 = 9;
const TAG_ADD_LEVEL_FILE: u8 = 10;
const TAG_DELETE_LEVEL_FILE: u8 = 11;
const MAX_FIELDS: usize = 1_000_000;
const MAX_COMPARATOR_BYTES: usize = 256;

#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameDescriptor {
    pub total_length: usize,
    pub payload_length: usize,
    pub field_count: usize,
    pub format_version: u16,
}

pub(crate) fn encode_edit_frame(
    edit: &VersionEdit,
    limits: &Limits,
    format_version: u16,
) -> Result<Vec<u8>> {
    validate_supported_version(format_version, "MANIFEST encoder")?;
    if edit.is_empty() {
        return Err(Error::invalid_argument(
            "version_edit",
            "пустой VersionEdit не кодируется",
        ));
    }
    if format_version == 1
        && (!edit.added_level_files.is_empty() || !edit.deleted_level_files.is_empty())
    {
        return Err(Error::UnsupportedFormat {
            component: "MANIFEST V1 leveled edit",
            found: 2,
            minimum_supported: 1,
            maximum_supported: 1,
        });
    }

    let mut fields = Vec::<(u8, Vec<u8>)>::new();
    if let Some(comparator) = &edit.comparator {
        if comparator.len() > MAX_COMPARATOR_BYTES {
            return Err(Error::ResourceLimit {
                resource: "manifest_comparator_bytes",
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
                .map_or(0, |value| value.get())
                .to_le_bytes()
                .to_vec(),
        ));
    }
    for file in &edit.added_files {
        fields.push((TAG_ADD_FILE, encode_table_meta(file, limits)?));
    }
    for number in &edit.deleted_files {
        fields.push((TAG_DELETE_FILE, number.to_le_bytes().to_vec()));
    }
    if format_version >= 2 {
        for file in &edit.added_level_files {
            fields.push((TAG_ADD_LEVEL_FILE, encode_level_file(file, limits)?));
        }
        for file in &edit.deleted_level_files {
            fields.push((TAG_DELETE_LEVEL_FILE, encode_level_ref(*file)?));
        }
    }
    if let Some(version) = edit.minimum_reader_version {
        fields.push((TAG_MINIMUM_READER_VERSION, version.to_le_bytes().to_vec()));
    }
    if let Some(flags) = edit.feature_flags {
        fields.push((TAG_FEATURE_FLAGS, flags.to_le_bytes().to_vec()));
    }
    if fields.len() > MAX_FIELDS {
        return Err(Error::ResourceLimit {
            resource: "manifest_field_count",
            limit: u64::try_from(MAX_FIELDS).unwrap_or(u64::MAX),
            observed: u64::try_from(fields.len()).unwrap_or(u64::MAX),
        });
    }

    let mut payload = Vec::new();
    payload.push(
        u8::try_from(format_version).map_err(|_| Error::ResourceLimit {
            resource: "manifest_format_version",
            limit: u64::from(u8::MAX),
            observed: u64::from(format_version),
        })?,
    );
    payload.extend_from_slice(&[0_u8; 3]);
    for (tag, value) in &fields {
        payload.push(*tag);
        payload.extend_from_slice(&[0_u8; 3]);
        let length = u32::try_from(value.len()).map_err(|_| Error::ResourceLimit {
            resource: "manifest_field_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(value.len()).unwrap_or(u64::MAX),
        })?;
        payload.extend_from_slice(&length.to_le_bytes());
        payload.extend_from_slice(value);
    }
    if payload.len() > limits.max_batch_size {
        return Err(Error::ResourceLimit {
            resource: "manifest_payload_length",
            limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
            observed: u64::try_from(payload.len()).unwrap_or(u64::MAX),
        });
    }
    let total = MANIFEST_FRAME_HEADER_SIZE
        .checked_add(payload.len())
        .ok_or(Error::ResourceLimit {
            resource: "manifest_frame_length",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    let total_u32 = u32::try_from(total).map_err(|_| Error::ResourceLimit {
        resource: "manifest_frame_length",
        limit: u64::from(u32::MAX),
        observed: u64::try_from(total).unwrap_or(u64::MAX),
    })?;
    let payload_u32 = u32::try_from(payload.len()).map_err(|_| Error::ResourceLimit {
        resource: "manifest_payload_length",
        limit: u64::from(u32::MAX),
        observed: u64::try_from(payload.len()).unwrap_or(u64::MAX),
    })?;
    let field_count = u32::try_from(fields.len()).map_err(|_| Error::ResourceLimit {
        resource: "manifest_field_count",
        limit: u64::from(u32::MAX),
        observed: u64::MAX,
    })?;

    let mut frame = vec![0_u8; MANIFEST_FRAME_HEADER_SIZE];
    frame[0..4].copy_from_slice(frame_magic(format_version)?);
    frame[4..8].copy_from_slice(&total_u32.to_le_bytes());
    frame[12] = RECORD_VERSION_EDIT;
    frame[13] = 0;
    frame[14..16].copy_from_slice(&format_version.to_le_bytes());
    frame[16..20].copy_from_slice(&field_count.to_le_bytes());
    frame[20..24].copy_from_slice(&payload_u32.to_le_bytes());
    frame[24..32].copy_from_slice(&0_u64.to_le_bytes());
    frame.extend_from_slice(&payload);
    let checksum = crc32c(&frame[12..]);
    frame[8..12].copy_from_slice(&checksum.to_le_bytes());
    Ok(frame)
}

pub(crate) fn decode_frame_header(
    input: &[u8; MANIFEST_FRAME_HEADER_SIZE],
    limits: &Limits,
    manifest_format_version: u16,
) -> Result<FrameDescriptor> {
    validate_supported_version(manifest_format_version, "MANIFEST frame")?;
    if &input[0..4] != frame_magic(manifest_format_version)? {
        return Err(Error::corruption("MANIFEST frame", "неверная сигнатура"));
    }
    if input[12] != RECORD_VERSION_EDIT || input[13] != 0 {
        return Err(Error::UnsupportedFormat {
            component: "MANIFEST record type",
            found: u32::from(input[12]),
            minimum_supported: u32::from(RECORD_VERSION_EDIT),
            maximum_supported: u32::from(RECORD_VERSION_EDIT),
        });
    }
    let version = read_u16(input, 14, "MANIFEST frame")?;
    if version != manifest_format_version {
        return Err(Error::UnsupportedFormat {
            component: "MANIFEST frame",
            found: u32::from(version),
            minimum_supported: u32::from(manifest_format_version),
            maximum_supported: u32::from(manifest_format_version),
        });
    }
    if read_u64(input, 24, "MANIFEST frame")? != 0 {
        return Err(Error::corruption(
            "MANIFEST frame",
            "reserved bytes должны быть нулевыми",
        ));
    }
    let total = usize::try_from(read_u32(input, 4, "MANIFEST frame")?).map_err(|_| {
        Error::ResourceLimit {
            resource: "manifest_frame_length",
            limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
            observed: u64::MAX,
        }
    })?;
    let payload = usize::try_from(read_u32(input, 20, "MANIFEST frame")?).map_err(|_| {
        Error::ResourceLimit {
            resource: "manifest_payload_length",
            limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
            observed: u64::MAX,
        }
    })?;
    let expected = MANIFEST_FRAME_HEADER_SIZE
        .checked_add(payload)
        .ok_or_else(|| Error::corruption("MANIFEST frame", "длина переполнена"))?;
    if total != expected || payload > limits.max_batch_size {
        return Err(Error::ResourceLimit {
            resource: "manifest_frame_length",
            limit: u64::try_from(MANIFEST_FRAME_HEADER_SIZE + limits.max_batch_size)
                .unwrap_or(u64::MAX),
            observed: u64::try_from(total).unwrap_or(u64::MAX),
        });
    }
    let field_count = usize::try_from(read_u32(input, 16, "MANIFEST frame")?).map_err(|_| {
        Error::ResourceLimit {
            resource: "manifest_field_count",
            limit: u64::try_from(MAX_FIELDS).unwrap_or(u64::MAX),
            observed: u64::MAX,
        }
    })?;
    if field_count == 0 || field_count > MAX_FIELDS {
        return Err(Error::ResourceLimit {
            resource: "manifest_field_count",
            limit: u64::try_from(MAX_FIELDS).unwrap_or(u64::MAX),
            observed: u64::try_from(field_count).unwrap_or(u64::MAX),
        });
    }
    Ok(FrameDescriptor {
        total_length: total,
        payload_length: payload,
        field_count,
        format_version: manifest_format_version,
    })
}

pub(crate) fn decode_complete_frame(
    header: &[u8; MANIFEST_FRAME_HEADER_SIZE],
    payload: &[u8],
    descriptor: FrameDescriptor,
    limits: &Limits,
) -> Result<VersionEdit> {
    if payload.len() != descriptor.payload_length {
        return Err(Error::corruption(
            "MANIFEST frame",
            "payload length не совпадает с заголовком",
        ));
    }
    let mut checksum_input = Vec::with_capacity(20 + payload.len());
    checksum_input.extend_from_slice(&header[12..]);
    checksum_input.extend_from_slice(payload);
    if crc32c(&checksum_input) != read_u32(header, 8, "MANIFEST frame")? {
        return Err(Error::corruption("MANIFEST frame", "CRC32C не совпадает"));
    }
    let payload_version = payload.first().copied().unwrap_or(0);
    if payload.len() < 4
        || u16::from(payload_version) != descriptor.format_version
        || payload[1..4] != [0_u8; 3]
    {
        return Err(Error::UnsupportedFormat {
            component: "VersionEdit",
            found: u32::from(payload_version),
            minimum_supported: u32::from(descriptor.format_version),
            maximum_supported: u32::from(descriptor.format_version),
        });
    }
    let mut edit = VersionEdit::default();
    let mut offset = 4_usize;
    for _ in 0..descriptor.field_count {
        let field_header_end = offset
            .checked_add(8)
            .ok_or_else(|| Error::corruption("VersionEdit", "offset переполнен"))?;
        if field_header_end > payload.len() {
            return Err(Error::corruption("VersionEdit", "field header усечён"));
        }
        let tag = payload[offset];
        if payload[offset + 1..offset + 4] != [0_u8; 3] {
            return Err(Error::corruption(
                "VersionEdit",
                "reserved field bytes не равны нулю",
            ));
        }
        let length =
            usize::try_from(read_u32(payload, offset + 4, "VersionEdit")?).map_err(|_| {
                Error::ResourceLimit {
                    resource: "manifest_field_length",
                    limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
                    observed: u64::MAX,
                }
            })?;
        let value_end = field_header_end
            .checked_add(length)
            .ok_or_else(|| Error::corruption("VersionEdit", "field range переполнен"))?;
        let value = payload
            .get(field_header_end..value_end)
            .ok_or_else(|| Error::corruption("VersionEdit", "field value усечено"))?;
        decode_field(&mut edit, tag, value, limits, descriptor.format_version)?;
        offset = value_end;
    }
    if offset != payload.len() || edit.is_empty() {
        return Err(Error::corruption(
            "VersionEdit",
            "число fields или trailing bytes неверны",
        ));
    }
    Ok(edit)
}

fn decode_field(
    edit: &mut VersionEdit,
    tag: u8,
    value: &[u8],
    limits: &Limits,
    format_version: u16,
) -> Result<()> {
    match tag {
        TAG_COMPARATOR => {
            if edit.comparator.is_some() || value.len() > MAX_COMPARATOR_BYTES {
                return Err(Error::corruption(
                    "VersionEdit",
                    "comparator повторён или oversized",
                ));
            }
            edit.comparator = Some(
                std::str::from_utf8(value)
                    .map_err(|_| Error::corruption("VersionEdit", "comparator не UTF-8"))?
                    .to_owned(),
            );
        }
        TAG_LOG_NUMBER => set_once_u64(&mut edit.log_number, value, "log_number")?,
        TAG_PREVIOUS_LOG_NUMBER => {
            if edit.previous_log_number.is_some() {
                return Err(Error::corruption(
                    "VersionEdit",
                    "previous_log_number повторяется",
                ));
            }
            let number = exact_u64(value, "previous_log_number")?;
            edit.previous_log_number = Some(if number == 0 { None } else { Some(number) });
        }
        TAG_NEXT_FILE_NUMBER => {
            set_once_u64(&mut edit.next_file_number, value, "next_file_number")?
        }
        TAG_LAST_SEQUENCE => {
            if edit.last_sequence.is_some() {
                return Err(Error::corruption(
                    "VersionEdit",
                    "last_sequence повторяется",
                ));
            }
            let sequence = exact_u64(value, "last_sequence")?;
            edit.last_sequence =
                Some(if sequence == 0 {
                    None
                } else {
                    Some(SequenceNumber::new(sequence).map_err(|_| {
                        Error::corruption("VersionEdit", "last_sequence равен нулю")
                    })?)
                });
        }
        TAG_ADD_FILE => edit.added_files.push(decode_table_meta(value, limits)?),
        TAG_DELETE_FILE => edit
            .deleted_files
            .push(exact_u64(value, "deleted_file_number")?),
        TAG_MINIMUM_READER_VERSION => {
            if edit.minimum_reader_version.is_some() || value.len() != 4 {
                return Err(Error::corruption(
                    "VersionEdit",
                    "minimum_reader_version повторён или неверной ширины",
                ));
            }
            edit.minimum_reader_version = Some(read_u32(value, 0, "VersionEdit")?);
        }
        TAG_FEATURE_FLAGS => set_once_u64(&mut edit.feature_flags, value, "feature_flags")?,
        TAG_ADD_LEVEL_FILE if format_version >= 2 => edit
            .added_level_files
            .push(decode_level_file(value, limits)?),
        TAG_DELETE_LEVEL_FILE if format_version >= 2 => {
            edit.deleted_level_files.push(decode_level_ref(value)?)
        }
        _ => {
            return Err(Error::UnsupportedFormat {
                component: "VersionEdit field tag",
                found: u32::from(tag),
                minimum_supported: u32::from(TAG_COMPARATOR),
                maximum_supported: u32::from(if format_version >= 2 {
                    TAG_DELETE_LEVEL_FILE
                } else {
                    TAG_FEATURE_FLAGS
                }),
            });
        }
    }
    Ok(())
}

fn encode_level_file(file: &LevelFileMeta, limits: &Limits) -> Result<Vec<u8>> {
    if file.level == 0 {
        return Err(Error::invalid_argument(
            "level",
            "leveled SSTable не может принадлежать L0",
        ));
    }
    let table = encode_table_meta(&file.table, limits)?;
    let mut output = Vec::with_capacity(4_usize.saturating_add(table.len()));
    output.extend_from_slice(&file.level.to_le_bytes());
    output.extend_from_slice(&table);
    Ok(output)
}

fn decode_level_file(value: &[u8], limits: &Limits) -> Result<LevelFileMeta> {
    if value.len() < 56 {
        return Err(Error::corruption(
            "VersionEdit leveled add file",
            "record усечён",
        ));
    }
    let level = read_u32(value, 0, "VersionEdit leveled add file")?;
    let table = decode_table_meta(&value[4..], limits)?;
    LevelFileMeta::new(level, table, limits)
        .map_err(|_| Error::corruption("VersionEdit leveled add file", "level недопустим"))
}

fn encode_level_ref(file: LevelFileRef) -> Result<Vec<u8>> {
    if file.level == 0 || file.file_number == 0 {
        return Err(Error::invalid_argument(
            "leveled_delete",
            "level и file_number должны быть ненулевыми",
        ));
    }
    let mut output = Vec::with_capacity(12);
    output.extend_from_slice(&file.level.to_le_bytes());
    output.extend_from_slice(&file.file_number.to_le_bytes());
    Ok(output)
}

fn decode_level_ref(value: &[u8]) -> Result<LevelFileRef> {
    if value.len() != 12 {
        return Err(Error::corruption(
            "VersionEdit leveled delete file",
            "record имеет неверную ширину",
        ));
    }
    LevelFileRef::new(
        read_u32(value, 0, "VersionEdit leveled delete file")?,
        read_u64(value, 4, "VersionEdit leveled delete file")?,
    )
    .map_err(|_| Error::corruption("VersionEdit leveled delete file", "reference недопустима"))
}

fn encode_table_meta(file: &TableMeta, limits: &Limits) -> Result<Vec<u8>> {
    file.validate(limits)?;
    let smallest =
        u32::try_from(file.smallest_user_key.len()).map_err(|_| Error::ResourceLimit {
            resource: "manifest_smallest_key_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(file.smallest_user_key.len()).unwrap_or(u64::MAX),
        })?;
    let largest = u32::try_from(file.largest_user_key.len()).map_err(|_| Error::ResourceLimit {
        resource: "manifest_largest_key_length",
        limit: u64::from(u32::MAX),
        observed: u64::try_from(file.largest_user_key.len()).unwrap_or(u64::MAX),
    })?;
    let mut output = Vec::with_capacity(
        52_usize
            .saturating_add(file.smallest_user_key.len())
            .saturating_add(file.largest_user_key.len()),
    );
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&file.file_number.to_le_bytes());
    output.extend_from_slice(&file.file_size.to_le_bytes());
    output.extend_from_slice(&smallest.to_le_bytes());
    output.extend_from_slice(&largest.to_le_bytes());
    output.extend_from_slice(&file.smallest_sequence.get().to_le_bytes());
    output.extend_from_slice(&file.largest_sequence.get().to_le_bytes());
    output.extend_from_slice(&file.entry_count.to_le_bytes());
    output.extend_from_slice(&file.smallest_user_key);
    output.extend_from_slice(&file.largest_user_key);
    Ok(output)
}

fn decode_table_meta(value: &[u8], limits: &Limits) -> Result<TableMeta> {
    if value.len() < 52 || read_u32(value, 0, "VersionEdit add file")? != 0 {
        return Err(Error::corruption(
            "VersionEdit add file",
            "record усечён или legacy level не равен 0",
        ));
    }
    let smallest = usize::try_from(read_u32(value, 20, "VersionEdit add file")?).map_err(|_| {
        Error::ResourceLimit {
            resource: "manifest_smallest_key_length",
            limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
            observed: u64::MAX,
        }
    })?;
    let largest = usize::try_from(read_u32(value, 24, "VersionEdit add file")?).map_err(|_| {
        Error::ResourceLimit {
            resource: "manifest_largest_key_length",
            limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
            observed: u64::MAX,
        }
    })?;
    if smallest > limits.max_key_size || largest > limits.max_key_size {
        return Err(Error::ResourceLimit {
            resource: "manifest_key_length",
            limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
            observed: u64::try_from(smallest.max(largest)).unwrap_or(u64::MAX),
        });
    }
    let smallest_end = 52_usize
        .checked_add(smallest)
        .ok_or_else(|| Error::corruption("VersionEdit add file", "key range переполнен"))?;
    let end = smallest_end
        .checked_add(largest)
        .ok_or_else(|| Error::corruption("VersionEdit add file", "key range переполнен"))?;
    if end != value.len() {
        return Err(Error::corruption(
            "VersionEdit add file",
            "record length не совпадает с key lengths",
        ));
    }
    let file = TableMeta {
        file_number: read_u64(value, 4, "VersionEdit add file")?,
        file_size: read_u64(value, 12, "VersionEdit add file")?,
        smallest_user_key: value[52..smallest_end].to_vec(),
        largest_user_key: value[smallest_end..end].to_vec(),
        smallest_sequence: SequenceNumber::new(read_u64(value, 28, "VersionEdit add file")?)
            .map_err(|_| {
                Error::corruption("VersionEdit add file", "smallest_sequence равен нулю")
            })?,
        largest_sequence: SequenceNumber::new(read_u64(value, 36, "VersionEdit add file")?)
            .map_err(|_| {
                Error::corruption("VersionEdit add file", "largest_sequence равен нулю")
            })?,
        entry_count: read_u64(value, 44, "VersionEdit add file")?,
    };
    file.validate(limits)?;
    Ok(file)
}

fn validate_supported_version(version: u16, component: &'static str) -> Result<()> {
    if !(MINIMUM_MANIFEST_FORMAT_VERSION..=MANIFEST_FORMAT_VERSION).contains(&version) {
        return Err(Error::UnsupportedFormat {
            component,
            found: u32::from(version),
            minimum_supported: u32::from(MINIMUM_MANIFEST_FORMAT_VERSION),
            maximum_supported: u32::from(MANIFEST_FORMAT_VERSION),
        });
    }
    Ok(())
}

fn frame_magic(version: u16) -> Result<&'static [u8; 4]> {
    match version {
        1 => Ok(FRAME_MAGIC_V1),
        2 => Ok(FRAME_MAGIC_V2),
        _ => Err(Error::UnsupportedFormat {
            component: "MANIFEST frame magic",
            found: u32::from(version),
            minimum_supported: u32::from(MINIMUM_MANIFEST_FORMAT_VERSION),
            maximum_supported: u32::from(MANIFEST_FORMAT_VERSION),
        }),
    }
}

fn set_once_u64(target: &mut Option<u64>, value: &[u8], name: &'static str) -> Result<()> {
    if target.is_some() {
        return Err(Error::corruption(
            "VersionEdit",
            format!("поле {name} повторяется"),
        ));
    }
    *target = Some(exact_u64(value, name)?);
    Ok(())
}

fn exact_u64(value: &[u8], name: &'static str) -> Result<u64> {
    if value.len() != 8 {
        return Err(Error::corruption(
            "VersionEdit",
            format!("поле {name} имеет неверную ширину"),
        ));
    }
    read_u64(value, 0, "VersionEdit")
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

    fn metadata(number: u64) -> Result<TableMeta> {
        Ok(TableMeta {
            file_number: number,
            file_size: 256,
            smallest_user_key: b"a".to_vec(),
            largest_user_key: b"z".to_vec(),
            smallest_sequence: SequenceNumber::FIRST,
            largest_sequence: SequenceNumber::FIRST,
            entry_count: 1,
        })
    }

    #[test]
    fn v1_rejects_leveled_fields() -> Result<()> {
        let limits = Limits::default();
        let edit = VersionEdit {
            added_level_files: vec![LevelFileMeta::new(1, metadata(10)?, &limits)?],
            ..VersionEdit::default()
        };
        assert!(encode_edit_frame(&edit, &limits, 1).is_err());
        Ok(())
    }

    #[test]
    fn v2_round_trips_leveled_fields() -> Result<()> {
        let limits = Limits::default();
        let edit = VersionEdit {
            added_level_files: vec![LevelFileMeta::new(2, metadata(10)?, &limits)?],
            deleted_level_files: vec![LevelFileRef::new(1, 9)?],
            ..VersionEdit::default()
        };
        let frame = encode_edit_frame(&edit, &limits, 2)?;
        let header: [u8; MANIFEST_FRAME_HEADER_SIZE] = frame[..MANIFEST_FRAME_HEADER_SIZE]
            .try_into()
            .map_err(|_| Error::corruption("test", "frame header"))?;
        let descriptor = decode_frame_header(&header, &limits, 2)?;
        let decoded = decode_complete_frame(
            &header,
            &frame[MANIFEST_FRAME_HEADER_SIZE..],
            descriptor,
            &limits,
        )?;
        assert_eq!(decoded, edit);
        Ok(())
    }
}
