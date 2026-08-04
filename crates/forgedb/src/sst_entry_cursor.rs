use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use forgedb_core::{
    DatabaseId, Error, InternalKey, InternalKeyCodec, Limits, Result, SequenceNumber, ValueType,
};
use forgedb_sstable::{
    BLOCK_TRAILER_SIZE, CHECKSUM_TYPE_CRC32C, COMPRESSION_NONE, SST_FOOTER_SIZE,
    SST_HEADER_SIZE, SSTABLE_FORMAT_VERSION, TableEntry, TableMeta,
};

use crate::entry_cursor::EntryCursor;

const SST_MAGIC: &[u8; 8] = b"FORGESST";
const FOOTER_MAGIC: &[u8; 8] = b"FGSSTEND";
const INDEX_MAGIC: &[u8; 4] = b"FIDX";
const INDEX_VERSION: u16 = 1;
const INDEX_HEADER_SIZE: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockHandle {
    offset: u64,
    length: u64,
}

#[derive(Clone, Copy, Debug)]
struct CursorLimits {
    max_key_size: usize,
    max_value_size: usize,
    max_index_entries: usize,
}

impl CursorLimits {
    fn from_limits(limits: &Limits) -> Self {
        Self {
            max_key_size: limits.max_key_size,
            max_value_size: limits.max_value_size,
            max_index_entries: limits.max_batch_operations,
        }
    }
}

/// Ленивый cursor SSTable V1: в памяти находятся index и только один data block.
#[derive(Debug)]
pub(crate) struct SstEntryCursor {
    path: PathBuf,
    file: File,
    metadata: TableMeta,
    limits: CursorLimits,
    verify_checksums: bool,
    data_handles: Vec<BlockHandle>,
    next_block: usize,
    block_entries: Vec<TableEntry>,
    position: usize,
    previous_block_last: Option<InternalKey>,
    entries_seen: u64,
    first_user_key: Option<Vec<u8>>,
    last_user_key: Option<Vec<u8>>,
    smallest_sequence: Option<SequenceNumber>,
    largest_sequence: Option<SequenceNumber>,
    finished: bool,
    #[cfg(test)]
    blocks_loaded: usize,
    #[cfg(test)]
    max_resident_entries: usize,
}

impl SstEntryCursor {
    pub(crate) fn open(
        path: &Path,
        expected_database_id: DatabaseId,
        metadata: &TableMeta,
        limits: &Limits,
        max_file_size: u64,
        verify_checksums: bool,
    ) -> Result<Self> {
        metadata.validate(limits)?;
        let physical_size = std::fs::metadata(path)
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

        let mut file = File::open(path)
            .map_err(|source| Error::io("open_sstable_cursor", Some(path.to_path_buf()), source))?;
        let header = read_exact_at(
            &mut file,
            path,
            0,
            SST_HEADER_SIZE,
            "read_sstable_header",
        )?;
        decode_header(
            path,
            &header,
            expected_database_id,
            metadata,
            verify_checksums,
        )?;

        let footer_size = u64::try_from(SST_FOOTER_SIZE).unwrap_or(u64::MAX);
        let footer_offset = physical_size
            .checked_sub(footer_size)
            .ok_or_else(|| corruption(path, "SSTable footer", "файл слишком мал"))?;
        let footer = read_exact_at(
            &mut file,
            path,
            footer_offset,
            SST_FOOTER_SIZE,
            "read_sstable_footer",
        )?;
        let (filter_handle, index_handle, properties_handle) = decode_footer(
            path,
            &footer,
            physical_size,
            footer_offset,
            verify_checksums,
        )?;

        let _filter = read_block_from_file(
            &mut file,
            path,
            filter_handle,
            "SSTable filter block",
            verify_checksums,
        )?;
        let index_payload = read_block_from_file(
            &mut file,
            path,
            index_handle,
            "SSTable index block",
            verify_checksums,
        )?;
        let _properties = read_block_from_file(
            &mut file,
            path,
            properties_handle,
            "SSTable properties block",
            verify_checksums,
        )?;
        let cursor_limits = CursorLimits::from_limits(limits);
        let index = decode_index(
            path,
            &index_payload,
            cursor_limits,
            physical_size,
            footer_offset,
        )?;
        let data_handles = index
            .into_iter()
            .map(|(_, handle)| handle)
            .collect::<Vec<_>>();

        let mut all_handles = Vec::with_capacity(data_handles.len().saturating_add(3));
        all_handles.extend(data_handles.iter().copied());
        all_handles.push(filter_handle);
        all_handles.push(index_handle);
        all_handles.push(properties_handle);
        ensure_non_overlapping(path, &all_handles)?;

        let mut cursor = Self {
            path: path.to_path_buf(),
            file,
            metadata: metadata.clone(),
            limits: cursor_limits,
            verify_checksums,
            data_handles,
            next_block: 0,
            block_entries: Vec::new(),
            position: 0,
            previous_block_last: None,
            entries_seen: 0,
            first_user_key: None,
            last_user_key: None,
            smallest_sequence: None,
            largest_sequence: None,
            finished: false,
            #[cfg(test)]
            blocks_loaded: 0,
            #[cfg(test)]
            max_resident_entries: 0,
        };
        cursor.load_next_block()?;
        Ok(cursor)
    }

    fn load_next_block(&mut self) -> Result<()> {
        while self.next_block < self.data_handles.len() {
            let handle = *self
                .data_handles
                .get(self.next_block)
                .ok_or(Error::InternalInvariant {
                    invariant: "I-SST-CURSOR-BLOCK-INDEX",
                    reason: "index data block cursor вышел за границы".to_owned(),
                })?;
            self.next_block = self.next_block.checked_add(1).ok_or(Error::ResourceLimit {
                resource: "sstable_cursor_block_index",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
            let payload = read_block_from_file(
                &mut self.file,
                &self.path,
                handle,
                "SSTable data block",
                self.verify_checksums,
            )?;
            let entries = decode_data_block(&self.path, &payload, self.limits)?;
            if entries.is_empty() {
                return Err(corruption(
                    &self.path,
                    "SSTable data block",
                    "data block не содержит entries",
                ));
            }
            if self
                .previous_block_last
                .as_ref()
                .zip(entries.first())
                .is_some_and(|(previous, first)| previous >= &first.internal_key)
            {
                return Err(corruption(
                    &self.path,
                    "SSTable ordering",
                    "internal keys не строго возрастают между blocks",
                ));
            }
            self.observe_block(&entries)?;
            self.previous_block_last = entries.last().map(|entry| entry.internal_key.clone());
            self.block_entries = entries;
            self.position = 0;
            #[cfg(test)]
            {
                self.blocks_loaded = self.blocks_loaded.checked_add(1).ok_or(
                    Error::ResourceLimit {
                        resource: "sstable_cursor_blocks_loaded",
                        limit: u64::MAX,
                        observed: u64::MAX,
                    },
                )?;
                self.max_resident_entries = self.max_resident_entries.max(self.block_entries.len());
            }
            return Ok(());
        }

        self.block_entries.clear();
        self.position = 0;
        self.validate_complete_metadata()?;
        self.finished = true;
        Ok(())
    }

    fn observe_block(&mut self, entries: &[TableEntry]) -> Result<()> {
        let block_count = u64::try_from(entries.len()).map_err(|_| Error::ResourceLimit {
            resource: "sstable_cursor_entry_count",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        self.entries_seen = self
            .entries_seen
            .checked_add(block_count)
            .ok_or(Error::ResourceLimit {
                resource: "sstable_cursor_entry_count",
                limit: self.metadata.entry_count,
                observed: u64::MAX,
            })?;
        if self.entries_seen > self.metadata.entry_count {
            return Err(corruption(
                &self.path,
                "SSTable properties",
                "фактическое число entries превышает MANIFEST",
            ));
        }

        if self.first_user_key.is_none() {
            self.first_user_key = entries
                .first()
                .map(|entry| entry.internal_key.user_key().to_vec());
        }
        self.last_user_key = entries
            .last()
            .map(|entry| entry.internal_key.user_key().to_vec());
        for entry in entries {
            let sequence = entry.internal_key.sequence();
            self.smallest_sequence = Some(
                self.smallest_sequence
                    .map_or(sequence, |current| current.min(sequence)),
            );
            self.largest_sequence = Some(
                self.largest_sequence
                    .map_or(sequence, |current| current.max(sequence)),
            );
        }
        Ok(())
    }

    fn validate_complete_metadata(&self) -> Result<()> {
        if self.entries_seen != self.metadata.entry_count {
            return Err(corruption(
                &self.path,
                "SSTable properties",
                "entry count не совпадает с MANIFEST",
            ));
        }
        if self.first_user_key.as_deref() != Some(self.metadata.smallest_user_key.as_slice())
            || self.last_user_key.as_deref() != Some(self.metadata.largest_user_key.as_slice())
        {
            return Err(corruption(
                &self.path,
                "SSTable properties",
                "key range не совпадает с MANIFEST",
            ));
        }
        if self.smallest_sequence != Some(self.metadata.smallest_sequence)
            || self.largest_sequence != Some(self.metadata.largest_sequence)
        {
            return Err(corruption(
                &self.path,
                "SSTable properties",
                "sequence range не совпадает с MANIFEST",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn blocks_loaded(&self) -> usize {
        self.blocks_loaded
    }

    #[cfg(test)]
    fn max_resident_entries(&self) -> usize {
        self.max_resident_entries
    }
}

impl EntryCursor for SstEntryCursor {
    fn current(&self) -> Option<&TableEntry> {
        if self.finished {
            None
        } else {
            self.block_entries.get(self.position)
        }
    }

    fn advance(&mut self) -> Result<()> {
        if self.current().is_none() {
            return Err(Error::InternalInvariant {
                invariant: "I-SST-CURSOR-ADVANCE",
                reason: "попытка advance завершённого SstEntryCursor".to_owned(),
            });
        }
        self.position = self.position.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "sstable_cursor_entry_position",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        if self.position >= self.block_entries.len() {
            self.load_next_block()?;
        }
        Ok(())
    }
}

fn read_exact_at(
    file: &mut File,
    path: &Path,
    offset: u64,
    length: usize,
    operation: &'static str,
) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|source| Error::io(operation, Some(path.to_path_buf()), source))?;
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)
        .map_err(|source| Error::io(operation, Some(path.to_path_buf()), source))?;
    Ok(bytes)
}

fn read_block_from_file(
    file: &mut File,
    path: &Path,
    handle: BlockHandle,
    component: &'static str,
    verify_checksums: bool,
) -> Result<Vec<u8>> {
    let length = usize::try_from(handle.length).map_err(|_| Error::ResourceLimit {
        resource: "sstable_block_length",
        limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        observed: handle.length,
    })?;
    let mut block = read_exact_at(file, path, handle.offset, length, "read_sstable_block")?;
    let payload_end = block
        .len()
        .checked_sub(BLOCK_TRAILER_SIZE)
        .ok_or_else(|| corruption(path, component, "block trailer отсутствует"))?;
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
        return Err(corruption(path, component, "reserved trailer не равен нулю"));
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
        let stored = read_u32(path, &block, payload_end + 4, component)?;
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
    block.truncate(payload_end);
    Ok(block)
}

fn decode_header(
    path: &Path,
    input: &[u8],
    expected_database_id: DatabaseId,
    metadata: &TableMeta,
    verify_checksums: bool,
) -> Result<()> {
    if input.len() != SST_HEADER_SIZE || input.get(0..8) != Some(SST_MAGIC.as_slice()) {
        return Err(corruption(path, "SSTable header", "неверная сигнатура или длина"));
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
    let id_bytes: [u8; 16] = input
        .get(16..32)
        .ok_or_else(|| corruption(path, "SSTable header", "database UUID усечён"))?
        .try_into()
        .map_err(|_| corruption(path, "SSTable header", "database UUID имеет неверную ширину"))?;
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
    if input.len() != SST_FOOTER_SIZE || input.get(56..64) != Some(FOOTER_MAGIC.as_slice()) {
        return Err(corruption(path, "SSTable footer", "неверная сигнатура или длина"));
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
    validate_handle(path, filter, file_size, footer_offset, "SSTable filter handle")?;
    validate_handle(path, index, file_size, footer_offset, "SSTable index handle")?;
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
    limits: CursorLimits,
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
            limit: u64::try_from(limits.max_index_entries).unwrap_or(u64::MAX),
            observed: u64::MAX,
        }
    })?;
    if count == 0 || count > limits.max_index_entries {
        return Err(Error::ResourceLimit {
            resource: "sstable_index_entries",
            limit: u64::try_from(limits.max_index_entries).unwrap_or(u64::MAX),
            observed: u64::try_from(count).unwrap_or(u64::MAX),
        });
    }

    let codec_limits = Limits {
        max_key_size: limits.max_key_size,
        max_value_size: limits.max_value_size,
        max_batch_operations: limits.max_index_entries,
        ..Limits::default()
    };
    let mut offset = INDEX_HEADER_SIZE;
    let mut entries = Vec::with_capacity(count);
    let mut previous: Option<InternalKey> = None;
    for _ in 0..count {
        let key_length = usize::try_from(read_u32(path, input, offset, "SSTable index")?)
            .map_err(|_| Error::ResourceLimit {
                resource: "sstable_index_key_length",
                limit: u64::MAX,
                observed: u64::MAX,
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
        let internal_key = InternalKeyCodec::decode(key_bytes, &codec_limits)?;
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
        validate_handle(path, handle, file_size, footer_offset, "SSTable data handle")?;
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

fn decode_data_block(
    path: &Path,
    payload: &[u8],
    limits: CursorLimits,
) -> Result<Vec<TableEntry>> {
    if payload.len() < 8 {
        return Err(corruption(path, "SSTable data block", "block слишком мал"));
    }
    let count_offset = payload
        .len()
        .checked_sub(4)
        .ok_or_else(|| corruption(path, "SSTable data block", "restart count отсутствует"))?;
    let restart_count = usize::try_from(read_u32(
        path,
        payload,
        count_offset,
        "SSTable data block",
    )?)
    .map_err(|_| Error::ResourceLimit {
        resource: "sstable_restart_count",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
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
        let restart = usize::try_from(read_u32(
            path,
            payload,
            byte_offset,
            "SSTable data block",
        )?)
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

    let codec_limits = Limits {
        max_key_size: limits.max_key_size,
        max_value_size: limits.max_value_size,
        max_batch_operations: limits.max_index_entries,
        ..Limits::default()
    };
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
        let internal_key = InternalKeyCodec::decode(&encoded_key, &codec_limits)?;
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
        return Err(corruption(path, component, "block handle выходит за границы"));
    }
    Ok(())
}

fn ensure_non_overlapping(path: &Path, handles: &[BlockHandle]) -> Result<()> {
    let mut sorted = handles.to_vec();
    sorted.sort_by_key(|handle| handle.offset);
    for pair in sorted.windows(2) {
        let left = pair
            .first()
            .copied()
            .ok_or(Error::InternalInvariant {
                invariant: "I-SST-HANDLE-WINDOW",
                reason: "окно handles не содержит left".to_owned(),
            })?;
        let right = pair
            .get(1)
            .copied()
            .ok_or(Error::InternalInvariant {
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

fn decode_varint32(
    path: &Path,
    input: &[u8],
    offset: &mut usize,
    limit: usize,
) -> Result<usize> {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_fs::NativeFileSystem;
    use forgedb_sstable::{TableBuildOptions, build_table, sst_file_name};

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-sst-cursor-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("TEMP"))
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn cursor_keeps_only_one_data_block_resident() -> Result<()> {
        let directory = temporary_directory("bounded")?;
        let identity = DatabaseId::random();
        let limits = Limits::default();
        let mut entries = Vec::new();
        for index in 0..160_u64 {
            entries.push(TableEntry::value(
                format!("key-{index:04}").into_bytes(),
                SequenceNumber::new(index + 1)?,
                vec![u8::try_from(index % 251).unwrap_or(0); 256],
                &limits,
            )?);
        }
        entries.sort_by(|left, right| left.internal_key.cmp(&right.internal_key));
        let metadata = build_table(
            &NativeFileSystem,
            &directory,
            &directory.join("TEMP"),
            identity,
            7,
            &entries,
            &limits,
            TableBuildOptions {
                data_block_size: 1024,
                restart_interval: 4,
                bloom_bits_per_key: 10,
            },
        )?;
        let path = directory.join(sst_file_name(7));
        let mut cursor = SstEntryCursor::open(
            &path,
            identity,
            &metadata,
            &limits,
            u64::MAX,
            true,
        )?;
        let mut observed = 0_usize;
        while cursor.current().is_some() {
            observed = observed.checked_add(1).ok_or(Error::ResourceLimit {
                resource: "test_observed_entries",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
            cursor.advance()?;
        }
        assert_eq!(observed, entries.len());
        assert!(cursor.blocks_loaded() > 1);
        assert!(cursor.max_resident_entries() < entries.len());
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
