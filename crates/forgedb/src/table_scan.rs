use std::cmp::Ordering;
use std::fs;
use std::path::Path;

use forgedb_core::{
    DatabaseId, Error, InternalKey, InternalKeyCodec, Limits, Result, SequenceNumber, ValueType,
};
use forgedb_sstable::{
    BLOCK_TRAILER_SIZE, CHECKSUM_TYPE_CRC32C, COMPRESSION_NONE, SST_FOOTER_SIZE, SST_HEADER_SIZE,
    SSTABLE_FORMAT_VERSION, TableEntry, TableMeta,
};

const SST_MAGIC: &[u8; 8] = b"FORGESST";
const FOOTER_MAGIC: &[u8; 8] = b"FGSSTEND";
const INDEX_MAGIC: &[u8; 4] = b"FIDX";
const INDEX_VERSION: u16 = 1;
const INDEX_HEADER_SIZE: usize = 12;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TableSnapshotLookup {
    Missing,
    Tombstone,
    Value(Vec<u8>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockHandle {
    offset: u64,
    length: u64,
}

pub(crate) fn read_entries(
    path: &Path,
    expected_database_id: DatabaseId,
    metadata: &TableMeta,
    limits: &Limits,
    max_file_size: u64,
    verify_checksums: bool,
) -> Result<Vec<TableEntry>> {
    metadata.validate(limits)?;
    let physical_size = fs::metadata(path)
        .map_err(|source| Error::io("stat_sstable", Some(path.to_path_buf()), source))?
        .len();
    if physical_size != metadata.file_size {
        return Err(corruption(
            path,
            "SSTable",
            "physical size не совпадает с MANIFEST",
        ));
    }
    if physical_size > max_file_size {
        return Err(Error::ResourceLimit {
            resource: "sstable_file_size",
            limit: max_file_size,
            observed: physical_size,
        });
    }
    if metadata.entry_count > max_file_size {
        return Err(Error::ResourceLimit {
            resource: "sstable_entry_count",
            limit: max_file_size,
            observed: metadata.entry_count,
        });
    }
    let expected_len = usize::try_from(physical_size).map_err(|_| Error::ResourceLimit {
        resource: "sstable_file_size",
        limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        observed: physical_size,
    })?;
    let bytes = fs::read(path)
        .map_err(|source| Error::io("read_sstable", Some(path.to_path_buf()), source))?;
    if bytes.len() != expected_len {
        return Err(corruption(
            path,
            "SSTable",
            "прочитанная длина не совпадает с metadata",
        ));
    }
    decode_header(
        path,
        &bytes,
        expected_database_id,
        metadata,
        verify_checksums,
    )?;
    let footer_offset = physical_size
        .checked_sub(u64::try_from(SST_FOOTER_SIZE).unwrap_or(u64::MAX))
        .ok_or_else(|| corruption(path, "SSTable footer", "файл слишком мал"))?;
    let footer_start = usize::try_from(footer_offset).map_err(|_| Error::ResourceLimit {
        resource: "sstable_footer_offset",
        limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        observed: footer_offset,
    })?;
    let footer = bytes
        .get(footer_start..)
        .ok_or_else(|| corruption(path, "SSTable footer", "footer отсутствует"))?;
    let (filter_handle, index_handle, properties_handle) =
        decode_footer(path, footer, physical_size, footer_offset, verify_checksums)?;

    let _filter = read_block(
        path,
        &bytes,
        filter_handle,
        "SSTable filter block",
        verify_checksums,
    )?;
    let index_payload = read_block(
        path,
        &bytes,
        index_handle,
        "SSTable index block",
        verify_checksums,
    )?;
    let _properties = read_block(
        path,
        &bytes,
        properties_handle,
        "SSTable properties block",
        verify_checksums,
    )?;
    let index = decode_index(path, index_payload, limits, physical_size, footer_offset)?;

    let mut all_handles = Vec::with_capacity(index.len().saturating_add(3));
    all_handles.extend(index.iter().map(|(_, handle)| *handle));
    all_handles.push(filter_handle);
    all_handles.push(index_handle);
    all_handles.push(properties_handle);
    ensure_non_overlapping(path, &all_handles)?;

    let capacity = usize::try_from(metadata.entry_count).map_err(|_| Error::ResourceLimit {
        resource: "sstable_entry_count",
        limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        observed: metadata.entry_count,
    })?;
    let mut entries = Vec::with_capacity(capacity);
    let mut previous: Option<InternalKey> = None;
    for (_, handle) in index {
        let payload = read_block(path, &bytes, handle, "SSTable data block", verify_checksums)?;
        for entry in decode_data_block(path, payload, limits)? {
            if previous
                .as_ref()
                .is_some_and(|previous_key| previous_key >= &entry.internal_key)
            {
                return Err(corruption(
                    path,
                    "SSTable ordering",
                    "internal keys не строго возрастают между blocks",
                ));
            }
            if entries.len() >= capacity {
                return Err(corruption(
                    path,
                    "SSTable properties",
                    "фактическое число entries превышает MANIFEST",
                ));
            }
            previous = Some(entry.internal_key.clone());
            entries.push(entry);
        }
    }
    validate_metadata(path, metadata, &entries)?;
    Ok(entries)
}

fn decode_header(
    path: &Path,
    bytes: &[u8],
    expected_database_id: DatabaseId,
    metadata: &TableMeta,
    verify_checksums: bool,
) -> Result<()> {
    let input = bytes
        .get(..SST_HEADER_SIZE)
        .ok_or_else(|| corruption(path, "SSTable header", "header усечён"))?;
    if input.get(0..8) != Some(SST_MAGIC.as_slice()) {
        return Err(corruption(path, "SSTable header", "неверная сигнатура"));
    }
    let version = read_u16(path, input, 8, "SSTable header")?;
    if version != SSTABLE_FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "SSTable",
            found: u32::from(version),
            minimum_supported: u32::from(SSTABLE_FORMAT_VERSION),
            maximum_supported: u32::from(SSTABLE_FORMAT_VERSION),
        });
    }
    if usize::from(read_u16(path, input, 10, "SSTable header")?) != SST_HEADER_SIZE {
        return Err(corruption(path, "SSTable header", "неверная header length"));
    }
    if read_u32(path, input, 12, "SSTable header")? != 0
        || read_u32(path, input, 56, "SSTable header")? != 0
    {
        return Err(corruption(
            path,
            "SSTable header",
            "reserved fields не равны нулю",
        ));
    }
    if verify_checksums
        && crc32c(
            input
                .get(..60)
                .ok_or_else(|| corruption(path, "SSTable header", "checksum domain усечён"))?,
        ) != read_u32(path, input, 60, "SSTable header")?
    {
        return Err(corruption(path, "SSTable header", "CRC32C не совпадает"));
    }
    let id_slice = input
        .get(16..32)
        .ok_or_else(|| corruption(path, "SSTable header", "database UUID усечён"))?;
    let id_bytes: [u8; 16] = id_slice.try_into().map_err(|_| {
        corruption(
            path,
            "SSTable header",
            "database UUID имеет неверную ширину",
        )
    })?;
    if DatabaseId::from_bytes(id_bytes) != expected_database_id {
        return Err(corruption(
            path,
            "SSTable header",
            "database UUID не совпадает",
        ));
    }
    if read_u64(path, input, 32, "SSTable header")? != metadata.file_number {
        return Err(corruption(
            path,
            "SSTable header",
            "file number не совпадает с MANIFEST",
        ));
    }
    if read_u64(path, input, 40, "SSTable header")? != metadata.smallest_sequence.get()
        || read_u64(path, input, 48, "SSTable header")? != metadata.largest_sequence.get()
    {
        return Err(corruption(
            path,
            "SSTable header",
            "sequence range не совпадает с MANIFEST",
        ));
    }
    Ok(())
}

fn decode_footer(
    path: &Path,
    input: &[u8],
    file_size: u64,
    footer_offset: u64,
    verify_checksums: bool,
) -> Result<(BlockHandle, BlockHandle, BlockHandle)> {
    if input.len() != SST_FOOTER_SIZE {
        return Err(corruption(path, "SSTable footer", "неверная длина"));
    }
    if input.get(56..64) != Some(FOOTER_MAGIC.as_slice()) {
        return Err(corruption(path, "SSTable footer", "неверная сигнатура"));
    }
    let version = read_u16(path, input, 48, "SSTable footer")?;
    if version != SSTABLE_FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "SSTable footer",
            found: u32::from(version),
            minimum_supported: u32::from(SSTABLE_FORMAT_VERSION),
            maximum_supported: u32::from(SSTABLE_FORMAT_VERSION),
        });
    }
    if read_u16(path, input, 50, "SSTable footer")? != 0 {
        return Err(corruption(path, "SSTable footer", "flags не равны нулю"));
    }
    if verify_checksums
        && crc32c(
            input
                .get(..52)
                .ok_or_else(|| corruption(path, "SSTable footer", "checksum domain усечён"))?,
        ) != read_u32(path, input, 52, "SSTable footer")?
    {
        return Err(corruption(path, "SSTable footer", "CRC32C не совпадает"));
    }
    let filter = decode_handle(path, input, 0, "SSTable filter handle")?;
    let index = decode_handle(path, input, 16, "SSTable index handle")?;
    let properties = decode_handle(path, input, 32, "SSTable properties handle")?;
    validate_handle(
        path,
        filter,
        file_size,
        footer_offset,
        "SSTable filter handle",
    )?;
    validate_handle(
        path,
        index,
        file_size,
        footer_offset,
        "SSTable index handle",
    )?;
    validate_handle(
        path,
        properties,
        file_size,
        footer_offset,
        "SSTable properties handle",
    )?;
    Ok((filter, index, properties))
}

fn decode_index(
    path: &Path,
    input: &[u8],
    limits: &Limits,
    file_size: u64,
    footer_offset: u64,
) -> Result<Vec<(InternalKey, BlockHandle)>> {
    if input.len() < INDEX_HEADER_SIZE || input.get(0..4) != Some(INDEX_MAGIC.as_slice()) {
        return Err(corruption(path, "SSTable index", "header отсутствует"));
    }
    let version = read_u16(path, input, 4, "SSTable index")?;
    if version != INDEX_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "SSTable index",
            found: u32::from(version),
            minimum_supported: u32::from(INDEX_VERSION),
            maximum_supported: u32::from(INDEX_VERSION),
        });
    }
    if read_u16(path, input, 6, "SSTable index")? != 0 {
        return Err(corruption(path, "SSTable index", "flags не равны нулю"));
    }
    let count = usize::try_from(read_u32(path, input, 8, "SSTable index")?).map_err(|_| {
        Error::ResourceLimit {
            resource: "sstable_index_entries",
            limit: u64::try_from(limits.max_batch_operations).unwrap_or(u64::MAX),
            observed: u64::MAX,
        }
    })?;
    if count == 0 || count > limits.max_batch_operations {
        return Err(Error::ResourceLimit {
            resource: "sstable_index_entries",
            limit: u64::try_from(limits.max_batch_operations).unwrap_or(u64::MAX),
            observed: u64::try_from(count).unwrap_or(u64::MAX),
        });
    }
    let mut offset = INDEX_HEADER_SIZE;
    let mut entries = Vec::with_capacity(count);
    let mut previous: Option<InternalKey> = None;
    for _ in 0..count {
        let key_length =
            usize::try_from(read_u32(path, input, offset, "SSTable index")?).map_err(|_| {
                Error::ResourceLimit {
                    resource: "sstable_index_key_length",
                    limit: u64::MAX,
                    observed: u64::MAX,
                }
            })?;
        offset = offset
            .checked_add(4)
            .ok_or_else(|| corruption(path, "SSTable index", "offset переполнен"))?;
        let max_encoded = limits
            .max_key_size
            .checked_add(25)
            .ok_or(Error::ResourceLimit {
                resource: "sstable_index_key_length",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        if key_length > max_encoded {
            return Err(Error::ResourceLimit {
                resource: "sstable_index_key_length",
                limit: u64::try_from(max_encoded).unwrap_or(u64::MAX),
                observed: u64::try_from(key_length).unwrap_or(u64::MAX),
            });
        }
        let key_end = offset
            .checked_add(key_length)
            .ok_or_else(|| corruption(path, "SSTable index", "key range переполнен"))?;
        let handle_end = key_end
            .checked_add(16)
            .ok_or_else(|| corruption(path, "SSTable index", "handle range переполнен"))?;
        let key_bytes = input
            .get(offset..key_end)
            .ok_or_else(|| corruption(path, "SSTable index", "separator key усечён"))?;
        let internal_key = InternalKeyCodec::decode(key_bytes, limits)?;
        if previous
            .as_ref()
            .is_some_and(|previous_key| previous_key >= &internal_key)
        {
            return Err(corruption(
                path,
                "SSTable index",
                "separator keys не строго возрастают",
            ));
        }
        let handle_bytes = input
            .get(key_end..handle_end)
            .ok_or_else(|| corruption(path, "SSTable index", "data handle усечён"))?;
        let handle = decode_handle(path, handle_bytes, 0, "SSTable data handle")?;
        validate_handle(
            path,
            handle,
            file_size,
            footer_offset,
            "SSTable data handle",
        )?;
        previous = Some(internal_key.clone());
        entries.push((internal_key, handle));
        offset = handle_end;
    }
    if offset != input.len() {
        return Err(corruption(
            path,
            "SSTable index",
            "после entries остались trailing bytes",
        ));
    }
    Ok(entries)
}

fn decode_data_block(path: &Path, payload: &[u8], limits: &Limits) -> Result<Vec<TableEntry>> {
    if payload.len() < 8 {
        return Err(corruption(path, "SSTable data block", "block слишком мал"));
    }
    let count_offset = payload
        .len()
        .checked_sub(4)
        .ok_or_else(|| corruption(path, "SSTable data block", "restart count отсутствует"))?;
    let restart_count =
        usize::try_from(read_u32(path, payload, count_offset, "SSTable data block")?).map_err(
            |_| Error::ResourceLimit {
                resource: "sstable_restart_count",
                limit: u64::MAX,
                observed: u64::MAX,
            },
        )?;
    if restart_count == 0 {
        return Err(corruption(
            path,
            "SSTable data block",
            "restart count равен нулю",
        ));
    }
    let restart_bytes = restart_count.checked_mul(4).ok_or(Error::ResourceLimit {
        resource: "sstable_restart_bytes",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    let restart_start = count_offset
        .checked_sub(restart_bytes)
        .ok_or_else(|| corruption(path, "SSTable data block", "restart array усечён"))?;
    let mut previous_restart = None;
    for index in 0..restart_count {
        let byte_offset = index
            .checked_mul(4)
            .and_then(|value| restart_start.checked_add(value))
            .ok_or_else(|| corruption(path, "SSTable data block", "restart offset переполнен"))?;
        let restart = usize::try_from(read_u32(path, payload, byte_offset, "SSTable data block")?)
            .map_err(|_| Error::ResourceLimit {
                resource: "sstable_restart_offset",
                limit: u64::try_from(restart_start).unwrap_or(u64::MAX),
                observed: u64::MAX,
            })?;
        if restart >= restart_start
            || previous_restart.is_some_and(|previous| restart <= previous)
            || (index == 0 && restart != 0)
        {
            return Err(corruption(
                path,
                "SSTable data block",
                "restart offsets нарушают ordering или bounds",
            ));
        }
        previous_restart = Some(restart);
    }

    let mut entries = Vec::new();
    let mut offset = 0_usize;
    let mut previous_encoded = Vec::<u8>::new();
    let mut previous_internal: Option<InternalKey> = None;
    while offset < restart_start {
        let shared = decode_varint32(path, payload, &mut offset, restart_start)?;
        let unshared = decode_varint32(path, payload, &mut offset, restart_start)?;
        let value_length = decode_varint32(path, payload, &mut offset, restart_start)?;
        if shared > previous_encoded.len() {
            return Err(corruption(
                path,
                "SSTable data block",
                "shared prefix больше предыдущего key",
            ));
        }
        let key_end = offset
            .checked_add(unshared)
            .ok_or_else(|| corruption(path, "SSTable data block", "key range переполнен"))?;
        let value_end = key_end
            .checked_add(value_length)
            .ok_or_else(|| corruption(path, "SSTable data block", "value range переполнен"))?;
        if value_end > restart_start {
            return Err(corruption(
                path,
                "SSTable data block",
                "entry выходит за restart array",
            ));
        }
        let encoded_length = shared.checked_add(unshared).ok_or(Error::ResourceLimit {
            resource: "sstable_internal_key_size",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        let max_encoded = limits
            .max_key_size
            .checked_add(25)
            .ok_or(Error::ResourceLimit {
                resource: "sstable_internal_key_size",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        if encoded_length > max_encoded || value_length > limits.max_value_size {
            return Err(Error::ResourceLimit {
                resource: "sstable_entry_size",
                limit: u64::try_from(max_encoded.max(limits.max_value_size)).unwrap_or(u64::MAX),
                observed: u64::try_from(encoded_length.max(value_length)).unwrap_or(u64::MAX),
            });
        }
        let suffix = payload
            .get(offset..key_end)
            .ok_or_else(|| corruption(path, "SSTable data block", "key suffix усечён"))?;
        let mut encoded_key = Vec::with_capacity(encoded_length);
        encoded_key.extend_from_slice(
            previous_encoded
                .get(..shared)
                .ok_or_else(|| corruption(path, "SSTable data block", "shared prefix усечён"))?,
        );
        encoded_key.extend_from_slice(suffix);
        let internal_key = InternalKeyCodec::decode(&encoded_key, limits)?;
        if previous_internal
            .as_ref()
            .is_some_and(|previous_key| previous_key >= &internal_key)
        {
            return Err(corruption(
                path,
                "SSTable data block",
                "internal keys не строго возрастают",
            ));
        }
        let value = payload
            .get(key_end..value_end)
            .ok_or_else(|| corruption(path, "SSTable data block", "value усечён"))?
            .to_vec();
        if internal_key.value_type() == ValueType::Deletion && !value.is_empty() {
            return Err(corruption(
                path,
                "SSTable data block",
                "tombstone содержит value",
            ));
        }
        previous_encoded = encoded_key;
        previous_internal = Some(internal_key.clone());
        entries.push(TableEntry {
            internal_key,
            value,
        });
        offset = value_end;
    }
    if offset != restart_start {
        return Err(corruption(
            path,
            "SSTable data block",
            "entry stream не заканчивается на restart array",
        ));
    }
    Ok(entries)
}

fn read_block<'a>(
    path: &Path,
    bytes: &'a [u8],
    handle: BlockHandle,
    component: &'static str,
    verify_checksums: bool,
) -> Result<&'a [u8]> {
    let start = usize::try_from(handle.offset).map_err(|_| Error::ResourceLimit {
        resource: "sstable_block_offset",
        limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        observed: handle.offset,
    })?;
    let length = usize::try_from(handle.length).map_err(|_| Error::ResourceLimit {
        resource: "sstable_block_length",
        limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        observed: handle.length,
    })?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| corruption(path, component, "block range переполнен"))?;
    let block = bytes
        .get(start..end)
        .ok_or_else(|| corruption(path, component, "block выходит за файл"))?;
    let payload_end = block
        .len()
        .checked_sub(BLOCK_TRAILER_SIZE)
        .ok_or_else(|| corruption(path, component, "block trailer отсутствует"))?;
    let payload = block
        .get(..payload_end)
        .ok_or_else(|| corruption(path, component, "payload отсутствует"))?;
    let compression = *block
        .get(payload_end)
        .ok_or_else(|| corruption(path, component, "compression byte отсутствует"))?;
    let checksum_type = *block
        .get(payload_end + 1)
        .ok_or_else(|| corruption(path, component, "checksum type отсутствует"))?;
    let reserved = block
        .get(payload_end + 2..payload_end + 4)
        .ok_or_else(|| corruption(path, component, "reserved trailer усечён"))?;
    if reserved.iter().any(|byte| *byte != 0) {
        return Err(corruption(
            path,
            component,
            "reserved trailer не равен нулю",
        ));
    }
    if compression != COMPRESSION_NONE {
        return Err(Error::UnsupportedFormat {
            component,
            found: u32::from(compression),
            minimum_supported: u32::from(COMPRESSION_NONE),
            maximum_supported: u32::from(COMPRESSION_NONE),
        });
    }
    if checksum_type != CHECKSUM_TYPE_CRC32C {
        return Err(Error::UnsupportedFormat {
            component,
            found: u32::from(checksum_type),
            minimum_supported: u32::from(CHECKSUM_TYPE_CRC32C),
            maximum_supported: u32::from(CHECKSUM_TYPE_CRC32C),
        });
    }
    if verify_checksums {
        let stored = read_u32(path, block, payload_end + 4, component)?;
        let checksum_end = payload_end
            .checked_add(4)
            .ok_or_else(|| corruption(path, component, "checksum domain переполнен"))?;
        let checksum_domain = block
            .get(..checksum_end)
            .ok_or_else(|| corruption(path, component, "checksum domain усечён"))?;
        if crc32c(checksum_domain) != stored {
            return Err(corruption(path, component, "CRC32C не совпадает"));
        }
    }
    Ok(payload)
}

fn decode_handle(
    path: &Path,
    input: &[u8],
    offset: usize,
    component: &'static str,
) -> Result<BlockHandle> {
    Ok(BlockHandle {
        offset: read_u64(path, input, offset, component)?,
        length: read_u64(path, input, offset + 8, component)?,
    })
}

fn validate_handle(
    path: &Path,
    handle: BlockHandle,
    file_size: u64,
    footer_offset: u64,
    component: &'static str,
) -> Result<()> {
    if handle.length < u64::try_from(BLOCK_TRAILER_SIZE).unwrap_or(u64::MAX) {
        return Err(corruption(path, component, "block length меньше trailer"));
    }
    let end = handle
        .offset
        .checked_add(handle.length)
        .ok_or_else(|| corruption(path, component, "block range переполнен"))?;
    if handle.offset < u64::try_from(SST_HEADER_SIZE).unwrap_or(u64::MAX)
        || end > footer_offset
        || end > file_size
    {
        return Err(corruption(
            path,
            component,
            "block handle выходит за границы",
        ));
    }
    Ok(())
}

fn ensure_non_overlapping(path: &Path, handles: &[BlockHandle]) -> Result<()> {
    let mut sorted = handles.to_vec();
    sorted.sort_by_key(|handle| handle.offset);
    for pair in sorted.windows(2) {
        let left = pair.first().copied().ok_or(Error::InternalInvariant {
            invariant: "I-SST-HANDLE-WINDOW",
            reason: "окно handles не содержит left".to_owned(),
        })?;
        let right = pair.get(1).copied().ok_or(Error::InternalInvariant {
            invariant: "I-SST-HANDLE-WINDOW",
            reason: "окно handles не содержит right".to_owned(),
        })?;
        let left_end = left
            .offset
            .checked_add(left.length)
            .ok_or_else(|| corruption(path, "SSTable footer", "block range переполнен"))?;
        if left_end > right.offset {
            return Err(corruption(
                path,
                "SSTable footer",
                "block ranges перекрываются",
            ));
        }
    }
    Ok(())
}

fn validate_metadata(path: &Path, metadata: &TableMeta, entries: &[TableEntry]) -> Result<()> {
    if u64::try_from(entries.len()).unwrap_or(u64::MAX) != metadata.entry_count {
        return Err(corruption(
            path,
            "SSTable properties",
            "entry count не совпадает с MANIFEST",
        ));
    }
    let first = entries
        .first()
        .ok_or_else(|| corruption(path, "SSTable", "таблица не содержит entries"))?;
    let last = entries
        .last()
        .ok_or_else(|| corruption(path, "SSTable", "таблица не содержит last entry"))?;
    if first.internal_key.user_key() != metadata.smallest_user_key
        || last.internal_key.user_key() != metadata.largest_user_key
    {
        return Err(corruption(
            path,
            "SSTable properties",
            "key range не совпадает с MANIFEST",
        ));
    }
    let smallest_sequence = entries
        .iter()
        .map(|entry| entry.internal_key.sequence())
        .min()
        .ok_or_else(|| corruption(path, "SSTable", "sequence range отсутствует"))?;
    let largest_sequence = entries
        .iter()
        .map(|entry| entry.internal_key.sequence())
        .max()
        .ok_or_else(|| corruption(path, "SSTable", "sequence range отсутствует"))?;
    if smallest_sequence != metadata.smallest_sequence
        || largest_sequence != metadata.largest_sequence
    {
        return Err(corruption(
            path,
            "SSTable properties",
            "sequence range не совпадает с MANIFEST",
        ));
    }
    Ok(())
}

fn decode_varint32(path: &Path, input: &[u8], offset: &mut usize, limit: usize) -> Result<usize> {
    let mut value = 0_u32;
    for shift in (0..35).step_by(7) {
        if *offset >= limit {
            return Err(corruption(path, "SSTable data block", "varint32 усечён"));
        }
        let byte = *input
            .get(*offset)
            .ok_or_else(|| corruption(path, "SSTable data block", "varint32 вне input"))?;
        *offset = offset
            .checked_add(1)
            .ok_or_else(|| corruption(path, "SSTable data block", "offset переполнен"))?;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return usize::try_from(value).map_err(|_| Error::ResourceLimit {
                resource: "sstable_varint32",
                limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
                observed: u64::from(value),
            });
        }
    }
    Err(corruption(
        path,
        "SSTable data block",
        "varint32 длиннее пяти байтов",
    ))
}

fn read_u16(path: &Path, input: &[u8], offset: usize, component: &'static str) -> Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| corruption(path, component, "u16 offset переполнен"))?;
    let bytes: [u8; 2] = input
        .get(offset..end)
        .ok_or_else(|| corruption(path, component, "u16 усечён"))?
        .try_into()
        .map_err(|_| corruption(path, component, "u16 имеет неверную ширину"))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(path: &Path, input: &[u8], offset: usize, component: &'static str) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| corruption(path, component, "u32 offset переполнен"))?;
    let bytes: [u8; 4] = input
        .get(offset..end)
        .ok_or_else(|| corruption(path, component, "u32 усечён"))?
        .try_into()
        .map_err(|_| corruption(path, component, "u32 имеет неверную ширину"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(path: &Path, input: &[u8], offset: usize, component: &'static str) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| corruption(path, component, "u64 offset переполнен"))?;
    let bytes: [u8; 8] = input
        .get(offset..end)
        .ok_or_else(|| corruption(path, component, "u64 усечён"))?
        .try_into()
        .map_err(|_| corruption(path, component, "u64 имеет неверную ширину"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn corruption(path: &Path, component: &'static str, reason: impl Into<String>) -> Error {
    Error::Corruption {
        file: Some(path.to_path_buf()),
        offset: None,
        component,
        reason: reason.into(),
    }
}

fn crc32c(bytes: &[u8]) -> u32 {
    const POLYNOMIAL: u32 = 0x82f6_3b78;
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (POLYNOMIAL & mask);
        }
    }
    !crc
}
