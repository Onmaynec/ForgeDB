//! SSTable V1 ForgeDB: immutable sorted tables, checksummed blocks, index,
//! Bloom filter, properties и fixed footer.

mod bloom;
mod cache;
mod checksum;

pub use cache::{BlockCache, CacheStats};

use std::cmp::Ordering;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bloom::BloomFilter;
use checksum::crc32c;
use forgedb_core::{
    ColumnFamilyId, DatabaseId, Error, InternalKey, InternalKeyCodec, Limits, Result,
    SequenceNumber, ValueType,
};
use forgedb_fs::{FileSystem, RandomAccessReader, WritableFile};

pub const SST_HEADER_SIZE: usize = 64;
pub const SST_FOOTER_SIZE: usize = 64;
pub const BLOCK_TRAILER_SIZE: usize = 8;
pub const SSTABLE_FORMAT_VERSION: u16 = 1;
pub const CHECKSUM_TYPE_CRC32C: u8 = 1;
pub const COMPRESSION_NONE: u8 = 0;

const SST_MAGIC: &[u8; 8] = b"FORGESST";
const FOOTER_MAGIC: &[u8; 8] = b"FGSSTEND";
const PROPERTIES_MAGIC: &[u8; 4] = b"FPRP";
const PROPERTIES_VERSION: u16 = 1;
const DEFAULT_COLUMN_FAMILY: u32 = 0;
const MAX_PROPERTY_COUNT: usize = 64;
const INDEX_HEADER_SIZE: usize = 8;
const INDEX_MAGIC: &[u8; 4] = b"FIDX";
const INDEX_VERSION: u16 = 1;

/// Имя установленного SSTable.
#[must_use]
pub fn sst_file_name(file_number: u64) -> String {
    format!("SST-{file_number:06}.sst")
}

/// Имя временного SSTable.
#[must_use]
pub fn temporary_sst_file_name(file_number: u64) -> String {
    format!("SST-{file_number:06}.tmp")
}

/// Разбирает только canonical имя установленного SSTable.
#[must_use]
pub fn parse_sst_file_name(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("SST-")?.strip_suffix(".sst")?;
    if digits.len() < 6 || digits.bytes().any(|byte| !byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Полная запись, передаваемая из immutable memtable в SSTable builder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableEntry {
    pub internal_key: InternalKey,
    pub value: Vec<u8>,
}

impl TableEntry {
    /// Создаёт value entry.
    pub fn value(
        user_key: impl Into<Vec<u8>>,
        sequence: SequenceNumber,
        value: impl Into<Vec<u8>>,
        limits: &Limits,
    ) -> Result<Self> {
        let value = value.into();
        limits.validate_value(&value)?;
        Ok(Self {
            internal_key: InternalKey::new(
                ColumnFamilyId::DEFAULT,
                user_key,
                sequence,
                ValueType::Value,
                limits,
            )?,
            value,
        })
    }

    /// Создаёт tombstone entry.
    pub fn tombstone(
        user_key: impl Into<Vec<u8>>,
        sequence: SequenceNumber,
        limits: &Limits,
    ) -> Result<Self> {
        Ok(Self {
            internal_key: InternalKey::new(
                ColumnFamilyId::DEFAULT,
                user_key,
                sequence,
                ValueType::Deletion,
                limits,
            )?,
            value: Vec::new(),
        })
    }
}

/// Настройки физического построения SSTable V1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableBuildOptions {
    pub data_block_size: usize,
    pub restart_interval: usize,
    pub bloom_bits_per_key: u16,
}

impl TableBuildOptions {
    pub fn validate(self) -> Result<()> {
        if !(1024..=16 * 1024 * 1024).contains(&self.data_block_size) {
            return Err(Error::invalid_argument(
                "data_block_size",
                "значение должно находиться в диапазоне 1024..=16777216",
            ));
        }
        if self.restart_interval == 0 {
            return Err(Error::invalid_argument(
                "restart_interval",
                "значение должно быть больше нуля",
            ));
        }
        if !(1..=64).contains(&self.bloom_bits_per_key) {
            return Err(Error::invalid_argument(
                "bloom_bits_per_key",
                "значение должно находиться в диапазоне 1..=64",
            ));
        }
        Ok(())
    }
}

/// Полный запрос на физическое построение одного SSTable V1.
///
/// Тип объединяет неизменяемые входы builder и не позволяет перепутать
/// каталог базы, временный каталог, identity или номер файла.
pub struct TableBuildRequest<'a> {
    database_directory: &'a Path,
    temporary_directory: &'a Path,
    database_id: DatabaseId,
    file_number: u64,
    entries: &'a [TableEntry],
    limits: &'a Limits,
    options: TableBuildOptions,
}

impl<'a> TableBuildRequest<'a> {
    /// Создаёт полностью типизированный запрос к SSTable builder.
    #[must_use]
    pub fn new(
        database_directory: &'a Path,
        temporary_directory: &'a Path,
        database_id: DatabaseId,
        file_number: u64,
        entries: &'a [TableEntry],
        limits: &'a Limits,
        options: TableBuildOptions,
    ) -> Self {
        Self {
            database_directory,
            temporary_directory,
            database_id,
            file_number,
            entries,
            limits,
            options,
        }
    }
}

/// Persisted metadata одного L0 SSTable, хранимая в MANIFEST.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableMeta {
    pub file_number: u64,
    pub file_size: u64,
    pub smallest_user_key: Vec<u8>,
    pub largest_user_key: Vec<u8>,
    pub smallest_sequence: SequenceNumber,
    pub largest_sequence: SequenceNumber,
    pub entry_count: u64,
}

impl TableMeta {
    /// Проверяет bounded metadata до использования путей или allocation.
    pub fn validate(&self, limits: &Limits) -> Result<()> {
        if self.file_number == 0 {
            return Err(Error::corruption(
                "SSTable metadata",
                "file_number равен нулю",
            ));
        }
        if self.file_size < u64::try_from(SST_HEADER_SIZE + SST_FOOTER_SIZE).unwrap_or(u64::MAX) {
            return Err(Error::corruption(
                "SSTable metadata",
                "file_size слишком мал",
            ));
        }
        limits.validate_key(&self.smallest_user_key)?;
        limits.validate_key(&self.largest_user_key)?;
        if self.smallest_user_key > self.largest_user_key {
            return Err(Error::corruption(
                "SSTable metadata",
                "smallest_user_key больше largest_user_key",
            ));
        }
        if self.smallest_sequence > self.largest_sequence {
            return Err(Error::corruption(
                "SSTable metadata",
                "smallest_sequence больше largest_sequence",
            ));
        }
        if self.entry_count == 0 {
            return Err(Error::corruption(
                "SSTable metadata",
                "entry_count равен нулю",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn may_contain_user_key(&self, key: &[u8]) -> bool {
        self.smallest_user_key.as_slice() <= key && key <= self.largest_user_key.as_slice()
    }
}

/// Результат point lookup внутри одного SSTable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TableLookup {
    Missing,
    Tombstone,
    Value(Vec<u8>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockHandle {
    offset: u64,
    length: u64,
}

impl BlockHandle {
    fn encode(self, output: &mut [u8]) -> Result<()> {
        if output.len() != 16 {
            return Err(Error::InternalInvariant {
                invariant: "I-SST-BLOCK-HANDLE-WIDTH",
                reason: "BlockHandle требует ровно 16 байт".to_owned(),
            });
        }
        output[0..8].copy_from_slice(&self.offset.to_le_bytes());
        output[8..16].copy_from_slice(&self.length.to_le_bytes());
        Ok(())
    }

    fn decode(input: &[u8], component: &'static str) -> Result<Self> {
        if input.len() != 16 {
            return Err(Error::corruption(
                component,
                "BlockHandle имеет неверную ширину",
            ));
        }
        Ok(Self {
            offset: read_u64(input, 0, component)?,
            length: read_u64(input, 8, component)?,
        })
    }

    fn validate(self, file_size: u64, footer_offset: u64, component: &'static str) -> Result<()> {
        if self.length < u64::try_from(BLOCK_TRAILER_SIZE).unwrap_or(u64::MAX) {
            return Err(Error::corruption(component, "block length меньше trailer"));
        }
        let end = self
            .offset
            .checked_add(self.length)
            .ok_or_else(|| Error::corruption(component, "block handle переполнен"))?;
        if self.offset < u64::try_from(SST_HEADER_SIZE).unwrap_or(u64::MAX)
            || end > footer_offset
            || end > file_size
        {
            return Err(Error::corruption(
                component,
                "block handle выходит за границы файла",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct IndexEntry {
    separator: InternalKey,
    handle: BlockHandle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TableProperties {
    file_number: u64,
    entry_count: u64,
    value_count: u64,
    tombstone_count: u64,
    smallest_internal_key: Vec<u8>,
    largest_internal_key: Vec<u8>,
    smallest_user_key: Vec<u8>,
    largest_user_key: Vec<u8>,
    smallest_sequence: u64,
    largest_sequence: u64,
    raw_key_bytes: u64,
    raw_value_bytes: u64,
    data_block_count: u64,
    index_size: u64,
    filter_size: u64,
    creation_unix_ms: u64,
}

/// Индексный SSTable reader: держит handle открытым, но не загружает весь SST в RAM.
#[derive(Clone)]
pub struct TableReader {
    path: PathBuf,
    reader: Arc<Mutex<Box<dyn RandomAccessReader>>>,
    file_size: u64,
    meta: TableMeta,
    index: Vec<IndexEntry>,
    filter: BloomFilter,
    limits: Limits,
}

impl std::fmt::Debug for TableReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TableReader")
            .field("path", &self.path)
            .field("file_size", &self.file_size)
            .field("index_entries", &self.index.len())
            .field("meta", &self.meta)
            .finish_non_exhaustive()
    }
}

impl TableReader {
    /// Открывает и проверяет SSTable, включая header/footer, checksums, index, filter и properties.
    pub fn open<F: FileSystem>(
        filesystem: &F,
        directory: &Path,
        expected_database_id: DatabaseId,
        expected_meta: &TableMeta,
        limits: &Limits,
        max_file_size: u64,
    ) -> Result<Self> {
        expected_meta.validate(limits)?;
        let path = directory.join(sst_file_name(expected_meta.file_number));
        let physical_size = filesystem.file_size(&path)?;
        if physical_size != expected_meta.file_size {
            return Err(Error::corruption(
                "SSTable",
                format!(
                    "размер файла {} не совпадает с MANIFEST {}",
                    physical_size, expected_meta.file_size
                ),
            ));
        }
        if physical_size > max_file_size {
            return Err(Error::ResourceLimit {
                resource: "sstable_file_size",
                limit: max_file_size,
                observed: physical_size,
            });
        }
        // Bounded random-access reads: validate the fixed header/footer and metadata
        // blocks without materializing the physical SSTable.
        let mut reader = filesystem.open_random_access(&path)?;
        let mut header_bytes = [0_u8; SST_HEADER_SIZE];
        read_exact_at(
            &mut reader,
            &path,
            0,
            &mut header_bytes,
            "read_sstable_header",
        )?;
        let header = decode_header(
            &header_bytes,
            expected_database_id,
            expected_meta.file_number,
        )?;
        if header.smallest_sequence != expected_meta.smallest_sequence
            || header.largest_sequence != expected_meta.largest_sequence
        {
            return Err(Error::corruption(
                "SSTable header",
                "sequence range не совпадает с MANIFEST",
            ));
        }
        let footer_offset = physical_size
            .checked_sub(u64::try_from(SST_FOOTER_SIZE).unwrap_or(u64::MAX))
            .ok_or_else(|| Error::corruption("SSTable footer", "файл слишком мал"))?;
        let mut footer_bytes = [0_u8; SST_FOOTER_SIZE];
        read_exact_at(
            &mut reader,
            &path,
            footer_offset,
            &mut footer_bytes,
            "read_sstable_footer",
        )?;
        let (filter_handle, index_handle, properties_handle) =
            decode_footer(&footer_bytes, physical_size, footer_offset)?;
        ensure_non_overlapping(&[filter_handle, index_handle, properties_handle])?;

        let filter_bytes = read_checked_block_at(
            &mut reader,
            &path,
            filter_handle,
            physical_size,
            footer_offset,
            "SSTable filter block",
        )?;
        let index_bytes = read_checked_block_at(
            &mut reader,
            &path,
            index_handle,
            physical_size,
            footer_offset,
            "SSTable index block",
        )?;
        let properties_bytes = read_checked_block_at(
            &mut reader,
            &path,
            properties_handle,
            physical_size,
            footer_offset,
            "SSTable properties block",
        )?;
        let filter_payload = filter_bytes.as_slice();
        let index_payload = index_bytes.as_slice();
        let properties_payload = properties_bytes.as_slice();
        let filter = BloomFilter::decode(filter_payload, limits.max_batch_size)?;
        let index = decode_index(index_payload, limits, physical_size, footer_offset)?;
        if index.is_empty() {
            return Err(Error::corruption(
                "SSTable index",
                "index не содержит блоков",
            ));
        }
        let properties = decode_properties(properties_payload, limits)?;
        validate_properties(
            &properties,
            expected_meta,
            index_handle.length,
            filter_handle.length,
            &header,
        )?;

        Ok(Self {
            path,
            reader: Arc::new(Mutex::new(Box::new(reader))),
            file_size: physical_size,
            meta: expected_meta.clone(),
            index,
            filter,
            limits: *limits,
        })
    }

    /// Возвращает point lookup для default column family.
    pub fn get(&self, user_key: &[u8]) -> Result<TableLookup> {
        self.limits.validate_key(user_key)?;
        if !self.meta.may_contain_user_key(user_key) || !self.filter.may_contain(user_key) {
            return Ok(TableLookup::Missing);
        }
        let candidate = self
            .index
            .iter()
            .find(|entry| entry.separator.user_key() >= user_key)
            .or_else(|| self.index.last());
        let Some(candidate) = candidate else {
            return Ok(TableLookup::Missing);
        };
        let payload = self.read_checked_block(candidate.handle, "SSTable data block")?;
        for entry in decode_data_block(&payload, &self.limits)? {
            match entry.internal_key.user_key().cmp(user_key) {
                Ordering::Less => continue,
                Ordering::Greater => return Ok(TableLookup::Missing),
                Ordering::Equal => {
                    return match entry.internal_key.value_type() {
                        ValueType::Deletion => Ok(TableLookup::Tombstone),
                        ValueType::Value => Ok(TableLookup::Value(entry.value)),
                    };
                }
            }
        }
        Ok(TableLookup::Missing)
    }

    /// Snapshot-aware indexed lookup. Reads only blocks whose index range can contain the key.
    ///
    /// Versions одного user key могут лежать в нескольких последовательных data blocks.
    /// Поиск продолжает чтение до первой версии, видимой на snapshot.
    pub fn get_at_cached(
        &self,
        user_key: &[u8],
        snapshot: Option<SequenceNumber>,
        cache: &BlockCache,
    ) -> Result<TableLookup> {
        self.limits.validate_key(user_key)?;
        let Some(snapshot) = snapshot else {
            return Ok(TableLookup::Missing);
        };
        let Some(first) = self.first_candidate_block(user_key) else {
            return Ok(TableLookup::Missing);
        };
        for (index, _) in self.index.iter().enumerate().skip(first) {
            let entries = self.cached_entries(index, cache)?;
            for entry in entries.iter() {
                match entry.internal_key.user_key().cmp(user_key) {
                    Ordering::Less => continue,
                    Ordering::Greater => return Ok(TableLookup::Missing),
                    Ordering::Equal if entry.internal_key.sequence() > snapshot => continue,
                    Ordering::Equal => {
                        return match entry.internal_key.value_type() {
                            ValueType::Deletion => Ok(TableLookup::Tombstone),
                            ValueType::Value => Ok(TableLookup::Value(entry.value.clone())),
                        };
                    }
                }
            }
        }
        Ok(TableLookup::Missing)
    }

    /// Newest persisted sequence ключа, если она строго новее `after`.
    /// Используется при optimistic transaction conflict validation без full-table materialization.
    pub fn latest_sequence_after_cached(
        &self,
        user_key: &[u8],
        after: Option<SequenceNumber>,
        cache: &BlockCache,
    ) -> Result<Option<SequenceNumber>> {
        self.limits.validate_key(user_key)?;
        let Some(first) = self.first_candidate_block(user_key) else {
            return Ok(None);
        };
        for (index, _) in self.index.iter().enumerate().skip(first) {
            let entries = self.cached_entries(index, cache)?;
            for entry in entries.iter() {
                match entry.internal_key.user_key().cmp(user_key) {
                    Ordering::Less => continue,
                    Ordering::Greater => return Ok(None),
                    Ordering::Equal => {
                        let sequence = entry.internal_key.sequence();
                        return Ok((after.is_none_or(|threshold| sequence > threshold))
                            .then_some(sequence));
                    }
                }
            }
        }
        Ok(None)
    }

    fn first_candidate_block(&self, user_key: &[u8]) -> Option<usize> {
        if !self.meta.may_contain_user_key(user_key) || !self.filter.may_contain(user_key) {
            return None;
        }
        let first = self
            .index
            .partition_point(|entry| entry.separator.user_key() < user_key);
        (first < self.index.len()).then_some(first)
    }

    fn cached_entries(
        &self,
        index: usize,
        cache: &BlockCache,
    ) -> Result<std::sync::Arc<Vec<TableEntry>>> {
        let handle = self
            .index
            .get(index)
            .ok_or(Error::InternalInvariant {
                invariant: "I-TABLE-BLOCK-INDEX",
                reason: "позиция блока вне проверенного SSTable index".to_owned(),
            })?
            .handle;
        cache.get_or_decode(self.meta.file_number, index, || {
            let payload = self.read_checked_block(handle, "SSTable data block")?;
            decode_data_block(&payload, &self.limits)
        })
    }

    /// Возвращает только валидированный физический блок по указанному offset.
    /// FD удерживается для Windows-safe retirement, а mutex сериализует seek/read.
    fn read_checked_block(&self, handle: BlockHandle, component: &'static str) -> Result<Vec<u8>> {
        let mut reader = self.reader.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-SST-READER-MUTEX",
            reason: "mutex random-access SST reader poisoned".to_owned(),
        })?;
        let footer_offset = self
            .file_size
            .checked_sub(u64::try_from(SST_FOOTER_SIZE).unwrap_or(u64::MAX))
            .ok_or_else(|| Error::corruption(component, "файл слишком мал"))?;
        read_checked_block_at(
            reader.as_mut(),
            &self.path,
            handle,
            self.file_size,
            footer_offset,
            component,
        )
    }

    #[must_use]
    pub fn meta(&self) -> &TableMeta {
        &self.meta
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Строит, синхронизирует и атомарно устанавливает один SSTable V1.
pub fn build_table<F: FileSystem>(
    filesystem: &F,
    request: TableBuildRequest<'_>,
) -> Result<TableMeta> {
    let TableBuildRequest {
        database_directory,
        temporary_directory,
        database_id,
        file_number,
        entries,
        limits,
        options,
    } = request;
    options.validate()?;
    limits.validate()?;
    if file_number == 0 {
        return Err(Error::invalid_argument(
            "file_number",
            "ноль зарезервирован",
        ));
    }
    if entries.is_empty() {
        return Err(Error::invalid_argument(
            "entries",
            "пустой SSTable не создаётся",
        ));
    }
    validate_entries(entries, limits)?;

    filesystem.create_dir_all(temporary_directory)?;
    let temp_path = temporary_directory.join(temporary_sst_file_name(file_number));
    let final_path = database_directory.join(sst_file_name(file_number));
    let smallest_sequence = entries
        .iter()
        .map(|entry| entry.internal_key.sequence())
        .min()
        .ok_or(Error::InternalInvariant {
            invariant: "I-SST-NONEMPTY-SEQUENCE",
            reason: "непустой набор entries не имеет sequence".to_owned(),
        })?;
    let largest_sequence = entries
        .iter()
        .map(|entry| entry.internal_key.sequence())
        .max()
        .ok_or(Error::InternalInvariant {
            invariant: "I-SST-NONEMPTY-SEQUENCE",
            reason: "непустой набор entries не имеет sequence".to_owned(),
        })?;
    let header = encode_header(
        database_id,
        file_number,
        smallest_sequence,
        largest_sequence,
    );
    let mut file = filesystem.create_writable(&temp_path)?;
    file.write_all(&header)
        .map_err(|source| Error::io("write_sstable_header", Some(temp_path.clone()), source))?;
    let mut offset = u64::try_from(SST_HEADER_SIZE).unwrap_or(u64::MAX);

    let mut data_builder = DataBlockBuilder::new(options.restart_interval);
    let mut index_entries = Vec::<(Vec<u8>, BlockHandle)>::new();
    let mut data_block_count = 0_u64;
    for entry in entries {
        let encoded_key = InternalKeyCodec::encode(&entry.internal_key, limits)?;
        if !data_builder.is_empty()
            && data_builder.projected_size(&encoded_key, &entry.value)? > options.data_block_size
        {
            let separator = data_builder.last_key().ok_or(Error::InternalInvariant {
                invariant: "I-SST-BLOCK-SEPARATOR",
                reason: "непустой data block не имеет последнего ключа".to_owned(),
            })?;
            let payload = data_builder.finish()?;
            let handle = write_block(&mut file, &temp_path, &payload, &mut offset)?;
            index_entries.push((separator, handle));
            data_block_count = data_block_count
                .checked_add(1)
                .ok_or(Error::ResourceLimit {
                    resource: "sstable_data_block_count",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?;
            data_builder = DataBlockBuilder::new(options.restart_interval);
        }
        data_builder.add(&encoded_key, &entry.value)?;
    }
    if !data_builder.is_empty() {
        let separator = data_builder.last_key().ok_or(Error::InternalInvariant {
            invariant: "I-SST-BLOCK-SEPARATOR",
            reason: "непустой data block не имеет последнего ключа".to_owned(),
        })?;
        let payload = data_builder.finish()?;
        let handle = write_block(&mut file, &temp_path, &payload, &mut offset)?;
        index_entries.push((separator, handle));
        data_block_count = data_block_count
            .checked_add(1)
            .ok_or(Error::ResourceLimit {
                resource: "sstable_data_block_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
    }

    let filter = BloomFilter::build(
        entries.iter().map(|entry| entry.internal_key.user_key()),
        options.bloom_bits_per_key,
    )?;
    let filter_payload = filter.encode();
    let filter_handle = write_block(&mut file, &temp_path, &filter_payload, &mut offset)?;

    let index_payload = encode_index(&index_entries)?;
    let index_handle = write_block(&mut file, &temp_path, &index_payload, &mut offset)?;

    let properties = build_properties(
        file_number,
        entries,
        data_block_count,
        index_handle.length,
        filter_handle.length,
        limits,
    )?;
    let properties_payload = encode_properties(&properties)?;
    let properties_handle = write_block(&mut file, &temp_path, &properties_payload, &mut offset)?;

    let footer = encode_footer(filter_handle, index_handle, properties_handle)?;
    file.write_all(&footer)
        .map_err(|source| Error::io("write_sstable_footer", Some(temp_path.clone()), source))?;
    offset = offset
        .checked_add(u64::try_from(footer.len()).unwrap_or(u64::MAX))
        .ok_or(Error::ResourceLimit {
            resource: "sstable_file_size",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    file.sync_all()
        .map_err(|source| Error::io("sync_sstable", Some(temp_path.clone()), source))?;
    drop(file);
    filesystem.rename_atomic(&temp_path, &final_path)?;
    filesystem.sync_directory(database_directory)?;

    let smallest_user_key = entries
        .first()
        .ok_or(Error::InternalInvariant {
            invariant: "I-SST-FIRST-ENTRY",
            reason: "непустой SSTable не имеет first entry".to_owned(),
        })?
        .internal_key
        .user_key()
        .to_vec();
    let largest_user_key = entries
        .last()
        .ok_or(Error::InternalInvariant {
            invariant: "I-SST-LAST-ENTRY",
            reason: "непустой SSTable не имеет last entry".to_owned(),
        })?
        .internal_key
        .user_key()
        .to_vec();
    let entry_count = u64::try_from(entries.len()).map_err(|_| Error::ResourceLimit {
        resource: "sstable_entry_count",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    Ok(TableMeta {
        file_number,
        file_size: offset,
        smallest_user_key,
        largest_user_key,
        smallest_sequence,
        largest_sequence,
        entry_count,
    })
}

#[derive(Clone, Copy, Debug)]
struct Header {
    smallest_sequence: SequenceNumber,
    largest_sequence: SequenceNumber,
}

fn encode_header(
    database_id: DatabaseId,
    file_number: u64,
    smallest_sequence: SequenceNumber,
    largest_sequence: SequenceNumber,
) -> [u8; SST_HEADER_SIZE] {
    let mut output = [0_u8; SST_HEADER_SIZE];
    output[0..8].copy_from_slice(SST_MAGIC);
    output[8..10].copy_from_slice(&SSTABLE_FORMAT_VERSION.to_le_bytes());
    output[10..12].copy_from_slice(&(SST_HEADER_SIZE as u16).to_le_bytes());
    output[12..16].copy_from_slice(&0_u32.to_le_bytes());
    output[16..32].copy_from_slice(database_id.as_bytes());
    output[32..40].copy_from_slice(&file_number.to_le_bytes());
    output[40..48].copy_from_slice(&smallest_sequence.get().to_le_bytes());
    output[48..56].copy_from_slice(&largest_sequence.get().to_le_bytes());
    output[56..60].copy_from_slice(&0_u32.to_le_bytes());
    let checksum = crc32c(&output[..60]);
    output[60..64].copy_from_slice(&checksum.to_le_bytes());
    output
}

fn decode_header(
    bytes: &[u8],
    expected_database_id: DatabaseId,
    expected_file_number: u64,
) -> Result<Header> {
    let input = bytes
        .get(..SST_HEADER_SIZE)
        .ok_or_else(|| Error::corruption("SSTable header", "заголовок усечён"))?;
    if input.get(0..8) != Some(SST_MAGIC.as_slice()) {
        return Err(Error::corruption("SSTable header", "неверная сигнатура"));
    }
    let version = read_u16(input, 8, "SSTable header")?;
    if version != SSTABLE_FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "SSTable",
            found: u32::from(version),
            minimum_supported: u32::from(SSTABLE_FORMAT_VERSION),
            maximum_supported: u32::from(SSTABLE_FORMAT_VERSION),
        });
    }
    if usize::from(read_u16(input, 10, "SSTable header")?) != SST_HEADER_SIZE {
        return Err(Error::corruption(
            "SSTable header",
            "неверная header_length",
        ));
    }
    if read_u32(input, 12, "SSTable header")? != 0 || read_u32(input, 56, "SSTable header")? != 0 {
        return Err(Error::corruption(
            "SSTable header",
            "reserved fields должны быть нулевыми",
        ));
    }
    let stored_checksum = read_u32(input, 60, "SSTable header")?;
    if crc32c(&input[..60]) != stored_checksum {
        return Err(Error::corruption("SSTable header", "CRC32C не совпадает"));
    }
    let mut id_bytes = [0_u8; 16];
    id_bytes.copy_from_slice(&input[16..32]);
    if DatabaseId::from_bytes(id_bytes) != expected_database_id {
        return Err(Error::corruption(
            "SSTable header",
            "database UUID не совпадает",
        ));
    }
    if read_u64(input, 32, "SSTable header")? != expected_file_number {
        return Err(Error::corruption(
            "SSTable header",
            "file_number не совпадает с именем/MANIFEST",
        ));
    }
    Ok(Header {
        smallest_sequence: SequenceNumber::new(read_u64(input, 40, "SSTable header")?)
            .map_err(|_| Error::corruption("SSTable header", "smallest_sequence равен нулю"))?,
        largest_sequence: SequenceNumber::new(read_u64(input, 48, "SSTable header")?)
            .map_err(|_| Error::corruption("SSTable header", "largest_sequence равен нулю"))?,
    })
}

fn encode_footer(
    filter: BlockHandle,
    index: BlockHandle,
    properties: BlockHandle,
) -> Result<[u8; SST_FOOTER_SIZE]> {
    let mut output = [0_u8; SST_FOOTER_SIZE];
    filter.encode(&mut output[0..16])?;
    index.encode(&mut output[16..32])?;
    properties.encode(&mut output[32..48])?;
    output[48..50].copy_from_slice(&SSTABLE_FORMAT_VERSION.to_le_bytes());
    output[50..52].copy_from_slice(&0_u16.to_le_bytes());
    let checksum = crc32c(&output[..52]);
    output[52..56].copy_from_slice(&checksum.to_le_bytes());
    output[56..64].copy_from_slice(FOOTER_MAGIC);
    Ok(output)
}

fn decode_footer(
    input: &[u8],
    file_size: u64,
    footer_offset: u64,
) -> Result<(BlockHandle, BlockHandle, BlockHandle)> {
    if input.len() != SST_FOOTER_SIZE {
        return Err(Error::corruption(
            "SSTable footer",
            "footer имеет неверную длину",
        ));
    }
    if input.get(56..64) != Some(FOOTER_MAGIC.as_slice()) {
        return Err(Error::corruption("SSTable footer", "неверная сигнатура"));
    }
    let version = read_u16(input, 48, "SSTable footer")?;
    if version != SSTABLE_FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "SSTable footer",
            found: u32::from(version),
            minimum_supported: u32::from(SSTABLE_FORMAT_VERSION),
            maximum_supported: u32::from(SSTABLE_FORMAT_VERSION),
        });
    }
    if read_u16(input, 50, "SSTable footer")? != 0 {
        return Err(Error::corruption(
            "SSTable footer",
            "flags должны быть нулевыми",
        ));
    }
    if crc32c(&input[..52]) != read_u32(input, 52, "SSTable footer")? {
        return Err(Error::corruption("SSTable footer", "CRC32C не совпадает"));
    }
    let filter = BlockHandle::decode(&input[0..16], "SSTable filter handle")?;
    let index = BlockHandle::decode(&input[16..32], "SSTable index handle")?;
    let properties = BlockHandle::decode(&input[32..48], "SSTable properties handle")?;
    filter.validate(file_size, footer_offset, "SSTable filter handle")?;
    index.validate(file_size, footer_offset, "SSTable index handle")?;
    properties.validate(file_size, footer_offset, "SSTable properties handle")?;
    Ok((filter, index, properties))
}

fn ensure_non_overlapping(handles: &[BlockHandle]) -> Result<()> {
    let mut ranges = handles.to_vec();
    ranges.sort_by_key(|handle| handle.offset);
    for pair in ranges.windows(2) {
        let left = pair[0];
        let right = pair[1];
        let left_end = left
            .offset
            .checked_add(left.length)
            .ok_or_else(|| Error::corruption("SSTable footer", "block range переполнен"))?;
        if left_end > right.offset {
            return Err(Error::corruption(
                "SSTable footer",
                "служебные блоки перекрываются",
            ));
        }
    }
    Ok(())
}

fn write_block<W: WritableFile>(
    file: &mut W,
    path: &Path,
    payload: &[u8],
    offset: &mut u64,
) -> Result<BlockHandle> {
    let mut trailer_prefix = [0_u8; 4];
    trailer_prefix[0] = COMPRESSION_NONE;
    trailer_prefix[1] = CHECKSUM_TYPE_CRC32C;
    let mut checksum_input = Vec::with_capacity(payload.len().saturating_add(4));
    checksum_input.extend_from_slice(payload);
    checksum_input.extend_from_slice(&trailer_prefix);
    let checksum = crc32c(&checksum_input);
    file.write_all(payload)
        .map_err(|source| Error::io("write_sstable_block", Some(path.into()), source))?;
    file.write_all(&trailer_prefix)
        .and_then(|_| file.write_all(&checksum.to_le_bytes()))
        .map_err(|source| Error::io("write_sstable_block_trailer", Some(path.into()), source))?;
    let length_usize =
        payload
            .len()
            .checked_add(BLOCK_TRAILER_SIZE)
            .ok_or(Error::ResourceLimit {
                resource: "sstable_block_length",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
    let length = u64::try_from(length_usize).map_err(|_| Error::ResourceLimit {
        resource: "sstable_block_length",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    let handle = BlockHandle {
        offset: *offset,
        length,
    };
    *offset = (*offset).checked_add(length).ok_or(Error::ResourceLimit {
        resource: "sstable_file_size",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    Ok(handle)
}

/// Полное чтение проверенного диапазона. Не допускает короткий read и silently
/// truncated files. Работает с FileSystem reader, не выполняя native I/O в обход
/// предоставленной реализации.
fn read_exact_at<R: Read + Seek + ?Sized>(
    reader: &mut R,
    path: &Path,
    offset: u64,
    output: &mut [u8],
    operation: &'static str,
) -> Result<()> {
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| Error::io(operation, Some(path.to_path_buf()), source))?;
    reader
        .read_exact(output)
        .map_err(|source| Error::io(operation, Some(path.to_path_buf()), source))
}

/// Сначала проверяет физические границы блока, затем CRC32C/trailer. В памяти
/// одновременно удерживается только один выбранный блок, а не весь SSTable.
fn read_checked_block_at<R: Read + Seek + ?Sized>(
    reader: &mut R,
    path: &Path,
    handle: BlockHandle,
    file_size: u64,
    footer_offset: u64,
    component: &'static str,
) -> Result<Vec<u8>> {
    handle.validate(file_size, footer_offset, component)?;
    let length = usize::try_from(handle.length).map_err(|_| Error::ResourceLimit {
        resource: "sstable_block_length",
        limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        observed: handle.length,
    })?;
    let mut buffer = vec![0_u8; length];
    read_exact_at(
        reader,
        path,
        handle.offset,
        &mut buffer,
        "read_sstable_block",
    )?;
    let relative_handle = BlockHandle {
        offset: 0,
        length: handle.length,
    };
    read_block(&buffer, relative_handle, component).map(<[u8]>::to_vec)
}

fn read_block<'a>(
    bytes: &'a [u8],
    handle: BlockHandle,
    component: &'static str,
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
        .ok_or_else(|| Error::corruption(component, "block range переполнен"))?;
    let block = bytes
        .get(start..end)
        .ok_or_else(|| Error::corruption(component, "block выходит за границы файла"))?;
    let payload_end = block
        .len()
        .checked_sub(BLOCK_TRAILER_SIZE)
        .ok_or_else(|| Error::corruption(component, "block trailer отсутствует"))?;
    let payload = &block[..payload_end];
    let compression = block[payload_end];
    let checksum_type = block[payload_end + 1];
    if block[payload_end + 2..payload_end + 4]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(Error::corruption(
            component,
            "reserved trailer bytes не равны нулю",
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
    let stored = read_u32(block, payload_end + 4, component)?;
    let mut checksum_input = Vec::with_capacity(payload.len().saturating_add(4));
    checksum_input.extend_from_slice(payload);
    checksum_input.extend_from_slice(&block[payload_end..payload_end + 4]);
    if crc32c(&checksum_input) != stored {
        return Err(Error::corruption(component, "CRC32C не совпадает"));
    }
    Ok(payload)
}

struct DataBlockBuilder {
    restart_interval: usize,
    entries_since_restart: usize,
    payload: Vec<u8>,
    restarts: Vec<u32>,
    previous_key: Vec<u8>,
    last_key: Option<Vec<u8>>,
}

impl DataBlockBuilder {
    fn new(restart_interval: usize) -> Self {
        Self {
            restart_interval,
            entries_since_restart: 0,
            payload: Vec::new(),
            restarts: Vec::new(),
            previous_key: Vec::new(),
            last_key: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.last_key.is_none()
    }

    fn last_key(&self) -> Option<Vec<u8>> {
        self.last_key.clone()
    }

    fn projected_size(&self, key: &[u8], value: &[u8]) -> Result<usize> {
        let shared = if self.entries_since_restart >= self.restart_interval {
            0
        } else {
            shared_prefix(&self.previous_key, key)
        };
        let unshared = key
            .len()
            .checked_sub(shared)
            .ok_or(Error::InternalInvariant {
                invariant: "I-SST-PREFIX-LENGTH",
                reason: "shared prefix больше key length".to_owned(),
            })?;
        let entry = varint32_len(shared)?
            .checked_add(varint32_len(unshared)?)
            .and_then(|size| size.checked_add(varint32_len(value.len()).ok()?))
            .and_then(|size| size.checked_add(unshared))
            .and_then(|size| size.checked_add(value.len()))
            .ok_or(Error::ResourceLimit {
                resource: "sstable_data_block_size",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        let restart_count = self
            .restarts
            .len()
            .checked_add(1)
            .ok_or(Error::ResourceLimit {
                resource: "sstable_restart_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        self.payload
            .len()
            .checked_add(entry)
            .and_then(|size| size.checked_add(restart_count.checked_mul(4)?))
            .and_then(|size| size.checked_add(4))
            .ok_or(Error::ResourceLimit {
                resource: "sstable_data_block_size",
                limit: u64::MAX,
                observed: u64::MAX,
            })
    }

    fn add(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        let restart =
            self.entries_since_restart >= self.restart_interval || self.last_key.is_none();
        let shared = if restart {
            let offset = u32::try_from(self.payload.len()).map_err(|_| Error::ResourceLimit {
                resource: "sstable_restart_offset",
                limit: u64::from(u32::MAX),
                observed: u64::try_from(self.payload.len()).unwrap_or(u64::MAX),
            })?;
            self.restarts.push(offset);
            self.entries_since_restart = 0;
            0
        } else {
            shared_prefix(&self.previous_key, key)
        };
        let unshared = key
            .len()
            .checked_sub(shared)
            .ok_or(Error::InternalInvariant {
                invariant: "I-SST-PREFIX-LENGTH",
                reason: "shared prefix больше key length".to_owned(),
            })?;
        encode_varint32(shared, &mut self.payload)?;
        encode_varint32(unshared, &mut self.payload)?;
        encode_varint32(value.len(), &mut self.payload)?;
        self.payload.extend_from_slice(&key[shared..]);
        self.payload.extend_from_slice(value);
        self.previous_key.clear();
        self.previous_key.extend_from_slice(key);
        self.last_key = Some(key.to_vec());
        self.entries_since_restart =
            self.entries_since_restart
                .checked_add(1)
                .ok_or(Error::ResourceLimit {
                    resource: "sstable_restart_interval",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?;
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<u8>> {
        if self.restarts.is_empty() {
            return Err(Error::InternalInvariant {
                invariant: "I-SST-RESTART-NONEMPTY",
                reason: "непустой data block не имеет restart point".to_owned(),
            });
        }
        let count = u32::try_from(self.restarts.len()).map_err(|_| Error::ResourceLimit {
            resource: "sstable_restart_count",
            limit: u64::from(u32::MAX),
            observed: u64::MAX,
        })?;
        for offset in self.restarts {
            self.payload.extend_from_slice(&offset.to_le_bytes());
        }
        self.payload.extend_from_slice(&count.to_le_bytes());
        Ok(self.payload)
    }
}

fn decode_data_block(payload: &[u8], limits: &Limits) -> Result<Vec<TableEntry>> {
    if payload.len() < 8 {
        return Err(Error::corruption("SSTable data block", "block слишком мал"));
    }
    let restart_count =
        usize::try_from(read_u32(payload, payload.len() - 4, "SSTable data block")?).map_err(
            |_| Error::ResourceLimit {
                resource: "sstable_restart_count",
                limit: u64::MAX,
                observed: u64::MAX,
            },
        )?;
    if restart_count == 0 {
        return Err(Error::corruption(
            "SSTable data block",
            "restart_count равен нулю",
        ));
    }
    let restart_bytes = restart_count.checked_mul(4).ok_or(Error::ResourceLimit {
        resource: "sstable_restart_bytes",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    let restart_start = payload
        .len()
        .checked_sub(4)
        .and_then(|value| value.checked_sub(restart_bytes))
        .ok_or_else(|| Error::corruption("SSTable data block", "restart array усечён"))?;
    let mut previous_offset = None;
    for index in 0..restart_count {
        let offset = usize::try_from(read_u32(
            payload,
            restart_start + index * 4,
            "SSTable data block",
        )?)
        .map_err(|_| Error::ResourceLimit {
            resource: "sstable_restart_offset",
            limit: u64::try_from(restart_start).unwrap_or(u64::MAX),
            observed: u64::MAX,
        })?;
        if offset >= restart_start || previous_offset.is_some_and(|previous| offset <= previous) {
            return Err(Error::corruption(
                "SSTable data block",
                "restart offsets не возрастают или выходят за payload",
            ));
        }
        if index == 0 && offset != 0 {
            return Err(Error::corruption(
                "SSTable data block",
                "первый restart offset должен быть нулём",
            ));
        }
        previous_offset = Some(offset);
    }

    let mut entries = Vec::new();
    let mut offset = 0_usize;
    let mut previous_key = Vec::<u8>::new();
    let mut previous_internal: Option<InternalKey> = None;
    while offset < restart_start {
        let shared = decode_varint32(payload, &mut offset, restart_start)?;
        let unshared = decode_varint32(payload, &mut offset, restart_start)?;
        let value_length = decode_varint32(payload, &mut offset, restart_start)?;
        if shared > previous_key.len() {
            return Err(Error::corruption(
                "SSTable data block",
                "shared prefix больше предыдущего ключа",
            ));
        }
        let key_end = offset
            .checked_add(unshared)
            .ok_or_else(|| Error::corruption("SSTable data block", "key range переполнен"))?;
        let value_end = key_end
            .checked_add(value_length)
            .ok_or_else(|| Error::corruption("SSTable data block", "value range переполнен"))?;
        if value_end > restart_start {
            return Err(Error::corruption(
                "SSTable data block",
                "entry выходит за restart array",
            ));
        }
        let encoded_length = shared.checked_add(unshared).ok_or(Error::ResourceLimit {
            resource: "sstable_internal_key_size",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        let max_encoded_key = limits
            .max_key_size
            .checked_add(25)
            .ok_or(Error::ResourceLimit {
                resource: "sstable_internal_key_size",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        if encoded_length > max_encoded_key || value_length > limits.max_value_size {
            return Err(Error::ResourceLimit {
                resource: "sstable_entry_size",
                limit: u64::try_from(max_encoded_key.max(limits.max_value_size))
                    .unwrap_or(u64::MAX),
                observed: u64::try_from(encoded_length.max(value_length)).unwrap_or(u64::MAX),
            });
        }
        let mut encoded_key = Vec::with_capacity(encoded_length);
        encoded_key.extend_from_slice(&previous_key[..shared]);
        encoded_key.extend_from_slice(&payload[offset..key_end]);
        let internal_key = InternalKeyCodec::decode(&encoded_key, limits)?;
        if previous_internal
            .as_ref()
            .is_some_and(|previous| previous >= &internal_key)
        {
            return Err(Error::corruption(
                "SSTable data block",
                "internal keys не строго возрастают",
            ));
        }
        let value = payload[key_end..value_end].to_vec();
        if internal_key.value_type() == ValueType::Deletion && !value.is_empty() {
            return Err(Error::corruption(
                "SSTable data block",
                "tombstone содержит value bytes",
            ));
        }
        previous_key = encoded_key;
        previous_internal = Some(internal_key.clone());
        entries.push(TableEntry {
            internal_key,
            value,
        });
        offset = value_end;
    }
    if offset != restart_start {
        return Err(Error::corruption(
            "SSTable data block",
            "entry stream не заканчивается на restart array",
        ));
    }
    Ok(entries)
}

fn encode_index(entries: &[(Vec<u8>, BlockHandle)]) -> Result<Vec<u8>> {
    let count = u32::try_from(entries.len()).map_err(|_| Error::ResourceLimit {
        resource: "sstable_index_entries",
        limit: u64::from(u32::MAX),
        observed: u64::MAX,
    })?;
    let mut output = Vec::new();
    output.extend_from_slice(INDEX_MAGIC);
    output.extend_from_slice(&INDEX_VERSION.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&count.to_le_bytes());
    for (key, handle) in entries {
        let key_length = u32::try_from(key.len()).map_err(|_| Error::ResourceLimit {
            resource: "sstable_index_key_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(key.len()).unwrap_or(u64::MAX),
        })?;
        output.extend_from_slice(&key_length.to_le_bytes());
        output.extend_from_slice(key);
        output.extend_from_slice(&handle.offset.to_le_bytes());
        output.extend_from_slice(&handle.length.to_le_bytes());
    }
    Ok(output)
}

fn decode_index(
    input: &[u8],
    limits: &Limits,
    file_size: u64,
    footer_offset: u64,
) -> Result<Vec<IndexEntry>> {
    if input.len() < INDEX_HEADER_SIZE + 4 {
        return Err(Error::corruption("SSTable index", "index header усечён"));
    }
    if input.get(0..4) != Some(INDEX_MAGIC.as_slice()) {
        return Err(Error::corruption("SSTable index", "неверная сигнатура"));
    }
    let version = read_u16(input, 4, "SSTable index")?;
    if version != INDEX_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "SSTable index",
            found: u32::from(version),
            minimum_supported: u32::from(INDEX_VERSION),
            maximum_supported: u32::from(INDEX_VERSION),
        });
    }
    if read_u16(input, 6, "SSTable index")? != 0 {
        return Err(Error::corruption(
            "SSTable index",
            "flags должны быть нулевыми",
        ));
    }
    let count = usize::try_from(read_u32(input, 8, "SSTable index")?).map_err(|_| {
        Error::ResourceLimit {
            resource: "sstable_index_entries",
            limit: u64::MAX,
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
    let mut offset = 12_usize;
    let mut entries = Vec::with_capacity(count);
    let mut previous: Option<InternalKey> = None;
    for _ in 0..count {
        let key_length =
            usize::try_from(read_u32(input, offset, "SSTable index")?).map_err(|_| {
                Error::ResourceLimit {
                    resource: "sstable_index_key_length",
                    limit: u64::MAX,
                    observed: u64::MAX,
                }
            })?;
        offset = offset
            .checked_add(4)
            .ok_or_else(|| Error::corruption("SSTable index", "offset переполнен"))?;
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
            .ok_or_else(|| Error::corruption("SSTable index", "key range переполнен"))?;
        let handle_end = key_end
            .checked_add(16)
            .ok_or_else(|| Error::corruption("SSTable index", "handle range переполнен"))?;
        if handle_end > input.len() {
            return Err(Error::corruption("SSTable index", "entry усечена"));
        }
        let key = InternalKeyCodec::decode(&input[offset..key_end], limits)?;
        if previous.as_ref().is_some_and(|value| value >= &key) {
            return Err(Error::corruption(
                "SSTable index",
                "separator keys не строго возрастают",
            ));
        }
        let handle = BlockHandle::decode(&input[key_end..handle_end], "SSTable data handle")?;
        handle.validate(file_size, footer_offset, "SSTable data handle")?;
        entries.push(IndexEntry {
            separator: key.clone(),
            handle,
        });
        previous = Some(key);
        offset = handle_end;
    }
    if offset != input.len() {
        return Err(Error::corruption(
            "SSTable index",
            "после index entries остались trailing bytes",
        ));
    }
    ensure_non_overlapping(&entries.iter().map(|entry| entry.handle).collect::<Vec<_>>())?;
    Ok(entries)
}

fn build_properties(
    file_number: u64,
    entries: &[TableEntry],
    data_block_count: u64,
    index_size: u64,
    filter_size: u64,
    limits: &Limits,
) -> Result<TableProperties> {
    let first = entries.first().ok_or(Error::InternalInvariant {
        invariant: "I-SST-PROPERTIES-FIRST",
        reason: "properties требуют непустой SSTable".to_owned(),
    })?;
    let last = entries.last().ok_or(Error::InternalInvariant {
        invariant: "I-SST-PROPERTIES-LAST",
        reason: "properties требуют непустой SSTable".to_owned(),
    })?;
    let mut value_count = 0_u64;
    let mut tombstone_count = 0_u64;
    let mut raw_key_bytes = 0_u64;
    let mut raw_value_bytes = 0_u64;
    let mut smallest_sequence = u64::MAX;
    let mut largest_sequence = 0_u64;
    for entry in entries {
        raw_key_bytes = raw_key_bytes
            .checked_add(
                u64::try_from(entry.internal_key.user_key().len()).map_err(|_| {
                    Error::ResourceLimit {
                        resource: "sstable_raw_key_bytes",
                        limit: u64::MAX,
                        observed: u64::MAX,
                    }
                })?,
            )
            .ok_or(Error::ResourceLimit {
                resource: "sstable_raw_key_bytes",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        raw_value_bytes = raw_value_bytes
            .checked_add(
                u64::try_from(entry.value.len()).map_err(|_| Error::ResourceLimit {
                    resource: "sstable_raw_value_bytes",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?,
            )
            .ok_or(Error::ResourceLimit {
                resource: "sstable_raw_value_bytes",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        match entry.internal_key.value_type() {
            ValueType::Value => value_count = value_count.saturating_add(1),
            ValueType::Deletion => tombstone_count = tombstone_count.saturating_add(1),
        }
        smallest_sequence = smallest_sequence.min(entry.internal_key.sequence().get());
        largest_sequence = largest_sequence.max(entry.internal_key.sequence().get());
    }
    Ok(TableProperties {
        file_number,
        entry_count: u64::try_from(entries.len()).map_err(|_| Error::ResourceLimit {
            resource: "sstable_entry_count",
            limit: u64::MAX,
            observed: u64::MAX,
        })?,
        value_count,
        tombstone_count,
        smallest_internal_key: InternalKeyCodec::encode(&first.internal_key, limits)?,
        largest_internal_key: InternalKeyCodec::encode(&last.internal_key, limits)?,
        smallest_user_key: first.internal_key.user_key().to_vec(),
        largest_user_key: last.internal_key.user_key().to_vec(),
        smallest_sequence,
        largest_sequence,
        raw_key_bytes,
        raw_value_bytes,
        data_block_count,
        index_size,
        filter_size,
        creation_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    })
}

fn encode_properties(properties: &TableProperties) -> Result<Vec<u8>> {
    let mut fields = Vec::<(u16, Vec<u8>)>::new();
    push_u16_field(&mut fields, 1, SSTABLE_FORMAT_VERSION);
    push_u64_field(&mut fields, 2, properties.file_number);
    push_u64_field(&mut fields, 3, properties.entry_count);
    push_u64_field(&mut fields, 4, properties.value_count);
    push_u64_field(&mut fields, 5, properties.tombstone_count);
    push_bytes_field(&mut fields, 6, &properties.smallest_internal_key);
    push_bytes_field(&mut fields, 7, &properties.largest_internal_key);
    push_bytes_field(&mut fields, 8, &properties.smallest_user_key);
    push_bytes_field(&mut fields, 9, &properties.largest_user_key);
    push_u64_field(&mut fields, 10, properties.smallest_sequence);
    push_u64_field(&mut fields, 11, properties.largest_sequence);
    push_u64_field(&mut fields, 12, properties.raw_key_bytes);
    push_u64_field(&mut fields, 13, properties.raw_value_bytes);
    push_u64_field(&mut fields, 14, properties.data_block_count);
    push_u64_field(&mut fields, 15, properties.index_size);
    push_u64_field(&mut fields, 16, properties.filter_size);
    push_u8_field(&mut fields, 17, COMPRESSION_NONE);
    push_u64_field(&mut fields, 18, properties.creation_unix_ms);
    push_bytes_field(
        &mut fields,
        19,
        forgedb_core::InternalKeyComparator::NAME.as_bytes(),
    );
    push_u32_field(&mut fields, 20, DEFAULT_COLUMN_FAMILY);
    push_u8_field(&mut fields, 21, CHECKSUM_TYPE_CRC32C);
    let count = u16::try_from(fields.len()).map_err(|_| Error::ResourceLimit {
        resource: "sstable_property_count",
        limit: u64::from(u16::MAX),
        observed: u64::MAX,
    })?;
    let mut output = Vec::new();
    output.extend_from_slice(PROPERTIES_MAGIC);
    output.extend_from_slice(&PROPERTIES_VERSION.to_le_bytes());
    output.extend_from_slice(&count.to_le_bytes());
    for (tag, value) in fields {
        output.extend_from_slice(&tag.to_le_bytes());
        let length = u32::try_from(value.len()).map_err(|_| Error::ResourceLimit {
            resource: "sstable_property_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(value.len()).unwrap_or(u64::MAX),
        })?;
        output.extend_from_slice(&length.to_le_bytes());
        output.extend_from_slice(&value);
    }
    Ok(output)
}

fn decode_properties(input: &[u8], limits: &Limits) -> Result<TableProperties> {
    if input.len() < 8 || input.get(0..4) != Some(PROPERTIES_MAGIC.as_slice()) {
        return Err(Error::corruption(
            "SSTable properties",
            "заголовок отсутствует или имеет неверную сигнатуру",
        ));
    }
    let version = read_u16(input, 4, "SSTable properties")?;
    if version != PROPERTIES_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "SSTable properties",
            found: u32::from(version),
            minimum_supported: u32::from(PROPERTIES_VERSION),
            maximum_supported: u32::from(PROPERTIES_VERSION),
        });
    }
    let count = usize::from(read_u16(input, 6, "SSTable properties")?);
    if count == 0 || count > MAX_PROPERTY_COUNT {
        return Err(Error::ResourceLimit {
            resource: "sstable_property_count",
            limit: u64::try_from(MAX_PROPERTY_COUNT).unwrap_or(u64::MAX),
            observed: u64::try_from(count).unwrap_or(u64::MAX),
        });
    }
    let mut offset = 8_usize;
    let mut values = std::collections::BTreeMap::<u16, Vec<u8>>::new();
    for _ in 0..count {
        let tag = read_u16(input, offset, "SSTable properties")?;
        let length =
            usize::try_from(read_u32(input, offset + 2, "SSTable properties")?).map_err(|_| {
                Error::ResourceLimit {
                    resource: "sstable_property_length",
                    limit: u64::MAX,
                    observed: u64::MAX,
                }
            })?;
        offset = offset
            .checked_add(6)
            .ok_or_else(|| Error::corruption("SSTable properties", "offset переполнен"))?;
        if length > limits.max_batch_size {
            return Err(Error::ResourceLimit {
                resource: "sstable_property_length",
                limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
                observed: u64::try_from(length).unwrap_or(u64::MAX),
            });
        }
        let end = offset
            .checked_add(length)
            .ok_or_else(|| Error::corruption("SSTable properties", "field range переполнен"))?;
        let value = input
            .get(offset..end)
            .ok_or_else(|| Error::corruption("SSTable properties", "field усечён"))?
            .to_vec();
        if values.insert(tag, value).is_some() {
            return Err(Error::corruption(
                "SSTable properties",
                "повторяющийся property tag",
            ));
        }
        offset = end;
    }
    if offset != input.len() {
        return Err(Error::corruption(
            "SSTable properties",
            "после TLV остались trailing bytes",
        ));
    }
    let format = field_u16(&values, 1, "format_version")?;
    if format != SSTABLE_FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "SSTable properties format",
            found: u32::from(format),
            minimum_supported: u32::from(SSTABLE_FORMAT_VERSION),
            maximum_supported: u32::from(SSTABLE_FORMAT_VERSION),
        });
    }
    if field_u8(&values, 17, "compression")? != COMPRESSION_NONE
        || field_u32(&values, 20, "column_family")? != DEFAULT_COLUMN_FAMILY
        || field_u8(&values, 21, "checksum_type")? != CHECKSUM_TYPE_CRC32C
        || field_bytes(&values, 19, "comparator")?
            != forgedb_core::InternalKeyComparator::NAME.as_bytes()
    {
        return Err(Error::corruption(
            "SSTable properties",
            "несовместимые comparator/compression/checksum/column family",
        ));
    }
    Ok(TableProperties {
        file_number: field_u64(&values, 2, "file_number")?,
        entry_count: field_u64(&values, 3, "entry_count")?,
        value_count: field_u64(&values, 4, "value_count")?,
        tombstone_count: field_u64(&values, 5, "tombstone_count")?,
        smallest_internal_key: field_bytes(&values, 6, "smallest_internal_key")?.to_vec(),
        largest_internal_key: field_bytes(&values, 7, "largest_internal_key")?.to_vec(),
        smallest_user_key: field_bytes(&values, 8, "smallest_user_key")?.to_vec(),
        largest_user_key: field_bytes(&values, 9, "largest_user_key")?.to_vec(),
        smallest_sequence: field_u64(&values, 10, "smallest_sequence")?,
        largest_sequence: field_u64(&values, 11, "largest_sequence")?,
        raw_key_bytes: field_u64(&values, 12, "raw_key_bytes")?,
        raw_value_bytes: field_u64(&values, 13, "raw_value_bytes")?,
        data_block_count: field_u64(&values, 14, "data_block_count")?,
        index_size: field_u64(&values, 15, "index_size")?,
        filter_size: field_u64(&values, 16, "filter_size")?,
        creation_unix_ms: field_u64(&values, 18, "creation_unix_ms")?,
    })
}

fn validate_properties(
    properties: &TableProperties,
    meta: &TableMeta,
    index_size: u64,
    filter_size: u64,
    header: &Header,
) -> Result<()> {
    if properties.file_number != meta.file_number
        || properties.entry_count != meta.entry_count
        || properties.smallest_user_key != meta.smallest_user_key
        || properties.largest_user_key != meta.largest_user_key
        || properties.smallest_sequence != meta.smallest_sequence.get()
        || properties.largest_sequence != meta.largest_sequence.get()
        || properties.smallest_sequence != header.smallest_sequence.get()
        || properties.largest_sequence != header.largest_sequence.get()
        || properties.index_size != index_size
        || properties.filter_size != filter_size
        || properties
            .value_count
            .saturating_add(properties.tombstone_count)
            != properties.entry_count
        || properties.data_block_count == 0
    {
        return Err(Error::corruption(
            "SSTable properties",
            "properties не согласованы с header/footer/MANIFEST",
        ));
    }
    Ok(())
}

fn validate_entries(entries: &[TableEntry], limits: &Limits) -> Result<()> {
    let mut previous: Option<&InternalKey> = None;
    for entry in entries {
        limits.validate_key(entry.internal_key.user_key())?;
        limits.validate_value(&entry.value)?;
        if entry.internal_key.column_family() != ColumnFamilyId::DEFAULT {
            return Err(Error::invalid_argument(
                "column_family",
                "SSTable V1 поддерживает только default column family",
            ));
        }
        if entry.internal_key.value_type() == ValueType::Deletion && !entry.value.is_empty() {
            return Err(Error::invalid_argument(
                "tombstone_value",
                "tombstone не должен содержать value bytes",
            ));
        }
        if previous.is_some_and(|value| value >= &entry.internal_key) {
            return Err(Error::invalid_argument(
                "entries",
                "internal keys должны быть строго отсортированы",
            ));
        }
        previous = Some(&entry.internal_key);
    }
    Ok(())
}

fn shared_prefix(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left_byte, right_byte)| left_byte == right_byte)
        .count()
}

fn encode_varint32(value: usize, output: &mut Vec<u8>) -> Result<()> {
    let mut value = u32::try_from(value).map_err(|_| Error::ResourceLimit {
        resource: "sstable_varint32",
        limit: u64::from(u32::MAX),
        observed: u64::try_from(value).unwrap_or(u64::MAX),
    })?;
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
    Ok(())
}

fn decode_varint32(input: &[u8], offset: &mut usize, limit: usize) -> Result<usize> {
    let mut value = 0_u32;
    for shift in (0..35).step_by(7) {
        if *offset >= limit {
            return Err(Error::corruption("SSTable data block", "varint32 усечён"));
        }
        let byte = *input
            .get(*offset)
            .ok_or_else(|| Error::corruption("SSTable data block", "varint32 вне input"))?;
        *offset = (*offset)
            .checked_add(1)
            .ok_or_else(|| Error::corruption("SSTable data block", "offset переполнен"))?;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return usize::try_from(value).map_err(|_| Error::ResourceLimit {
                resource: "sstable_varint32",
                limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
                observed: u64::from(value),
            });
        }
    }
    Err(Error::corruption(
        "SSTable data block",
        "varint32 длиннее пяти байтов",
    ))
}

fn varint32_len(value: usize) -> Result<usize> {
    let value = u32::try_from(value).map_err(|_| Error::ResourceLimit {
        resource: "sstable_varint32",
        limit: u64::from(u32::MAX),
        observed: u64::try_from(value).unwrap_or(u64::MAX),
    })?;
    Ok(match value {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0x0fff_ffff => 4,
        _ => 5,
    })
}

fn push_u8_field(fields: &mut Vec<(u16, Vec<u8>)>, tag: u16, value: u8) {
    fields.push((tag, vec![value]));
}

fn push_u16_field(fields: &mut Vec<(u16, Vec<u8>)>, tag: u16, value: u16) {
    fields.push((tag, value.to_le_bytes().to_vec()));
}

fn push_u32_field(fields: &mut Vec<(u16, Vec<u8>)>, tag: u16, value: u32) {
    fields.push((tag, value.to_le_bytes().to_vec()));
}

fn push_u64_field(fields: &mut Vec<(u16, Vec<u8>)>, tag: u16, value: u64) {
    fields.push((tag, value.to_le_bytes().to_vec()));
}

fn push_bytes_field(fields: &mut Vec<(u16, Vec<u8>)>, tag: u16, value: &[u8]) {
    fields.push((tag, value.to_vec()));
}

fn field_bytes<'a>(
    fields: &'a std::collections::BTreeMap<u16, Vec<u8>>,
    tag: u16,
    name: &'static str,
) -> Result<&'a [u8]> {
    fields
        .get(&tag)
        .map(Vec::as_slice)
        .ok_or_else(|| Error::corruption("SSTable properties", format!("поле {name} отсутствует")))
}

fn field_u8(
    fields: &std::collections::BTreeMap<u16, Vec<u8>>,
    tag: u16,
    name: &'static str,
) -> Result<u8> {
    let value = field_bytes(fields, tag, name)?;
    if value.len() != 1 {
        return Err(Error::corruption(
            "SSTable properties",
            format!("поле {name} имеет неверную ширину"),
        ));
    }
    Ok(value[0])
}

fn field_u16(
    fields: &std::collections::BTreeMap<u16, Vec<u8>>,
    tag: u16,
    name: &'static str,
) -> Result<u16> {
    let value = field_bytes(fields, tag, name)?;
    if value.len() != 2 {
        return Err(Error::corruption(
            "SSTable properties",
            format!("поле {name} имеет неверную ширину"),
        ));
    }
    read_u16(value, 0, "SSTable properties")
}

fn field_u32(
    fields: &std::collections::BTreeMap<u16, Vec<u8>>,
    tag: u16,
    name: &'static str,
) -> Result<u32> {
    let value = field_bytes(fields, tag, name)?;
    if value.len() != 4 {
        return Err(Error::corruption(
            "SSTable properties",
            format!("поле {name} имеет неверную ширину"),
        ));
    }
    read_u32(value, 0, "SSTable properties")
}

fn field_u64(
    fields: &std::collections::BTreeMap<u16, Vec<u8>>,
    tag: u16,
    name: &'static str,
) -> Result<u64> {
    let value = field_bytes(fields, tag, name)?;
    if value.len() != 8 {
        return Err(Error::corruption(
            "SSTable properties",
            format!("поле {name} имеет неверную ширину"),
        ));
    }
    read_u64(value, 0, "SSTable properties")
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
    use std::fs;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_fs::NativeFileSystem;

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-sstable-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("TEMP"))
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }


    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    struct CountingReader {
        file: fs::File,
        bytes_read: Arc<AtomicUsize>,
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let size = self.file.read(buffer)?;
            self.bytes_read.fetch_add(size, AtomicOrdering::Relaxed);
            Ok(size)
        }
    }

    impl Seek for CountingReader {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.file.seek(pos)
        }
    }

    #[derive(Clone)]
    struct CountingFileSystem {
        bytes_read: Arc<AtomicUsize>,
    }

    impl FileSystem for CountingFileSystem {
        type SequentialReader = fs::File;
        type RandomAccessReader = CountingReader;
        type WritableFile = fs::File;
        type LockGuard = forgedb_fs::NativeLockGuard;

        fn create_dir_all(&self, path: &Path) -> Result<()> {
            NativeFileSystem.create_dir_all(path)
        }

        fn open_sequential(&self, path: &Path) -> Result<Self::SequentialReader> {
            NativeFileSystem.open_sequential(path)
        }

        fn open_random_access(&self, path: &Path) -> Result<Self::RandomAccessReader> {
            Ok(CountingReader {
                file: NativeFileSystem.open_random_access(path)?,
                bytes_read: Arc::clone(&self.bytes_read),
            })
        }

        fn create_writable(&self, path: &Path) -> Result<Self::WritableFile> {
            NativeFileSystem.create_writable(path)
        }

        fn create_new_writable(&self, path: &Path) -> Result<Self::WritableFile> {
            NativeFileSystem.create_new_writable(path)
        }

        fn open_appendable(&self, path: &Path) -> Result<Self::WritableFile> {
            NativeFileSystem.open_appendable(path)
        }

        fn rename_atomic(&self, from: &Path, to: &Path) -> Result<()> {
            NativeFileSystem.rename_atomic(from, to)
        }

        fn remove_file(&self, path: &Path) -> Result<()> {
            NativeFileSystem.remove_file(path)
        }

        fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
            NativeFileSystem.list_dir(path)
        }

        fn file_size(&self, path: &Path) -> Result<u64> {
            NativeFileSystem.file_size(path)
        }

        fn truncate_file(&self, path: &Path, length: u64) -> Result<()> {
            NativeFileSystem.truncate_file(path, length)
        }

        fn sync_directory(&self, path: &Path) -> Result<()> {
            NativeFileSystem.sync_directory(path)
        }

        fn lock_exclusive(&self, path: &Path) -> Result<Self::LockGuard> {
            NativeFileSystem.lock_exclusive(path)
        }
    }

    #[test]
    fn lazy_open_does_not_read_entire_sstable_and_cache_hits_avoid_io() -> Result<()> {
        let directory = temporary_directory("bounded-open")?;
        let fs = CountingFileSystem {
            bytes_read: Arc::new(AtomicUsize::new(0)),
        };
        let limits = Limits::default();
        let id = DatabaseId::random();
        let mut records = Vec::new();
        for n in 0..96_u64 {
            records.push(TableEntry::value(
                format!("key-{n:04}").into_bytes(),
                SequenceNumber::new(n + 1)?,
                vec![u8::try_from(n).unwrap_or_default(); 700],
                &limits,
            )?);
        }
        let meta = build_table(
            &NativeFileSystem,
            TableBuildRequest::new(
                &directory, &directory.join("TEMP"), id, 31, &records, &limits,
                TableBuildOptions {
                    data_block_size: 1024,
                    restart_interval: 4,
                    bloom_bits_per_key: 10,
                },
            ),
        )?;
        let reader = TableReader::open(
            &fs, &directory, id, &meta, &limits, 16 * 1024 * 1024,
        )?;
        let opened = fs.bytes_read.load(AtomicOrdering::Relaxed);
        assert!(u64::try_from(opened).unwrap_or(u64::MAX) < meta.file_size / 2);
        let cache = BlockCache::new(64 * 1024);
        let seq = Some(SequenceNumber::new(51)?);
        assert_eq!(
            reader.get_at_cached(b"key-0050", seq, &cache)?,
            TableLookup::Value(vec![50; 700])
        );
        let after_first = fs.bytes_read.load(AtomicOrdering::Relaxed);
        assert!(after_first > opened);
        assert_eq!(
            reader.get_at_cached(b"key-0050", seq, &cache)?,
            TableLookup::Value(vec![50; 700])
        );
        assert_eq!(fs.bytes_read.load(AtomicOrdering::Relaxed), after_first);
        drop(reader);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    fn entries(limits: &Limits) -> Result<Vec<TableEntry>> {
        let mut entries = vec![
            TableEntry::value(
                b"alpha".to_vec(),
                SequenceNumber::new(3)?,
                b"one".to_vec(),
                limits,
            )?,
            TableEntry::tombstone(b"beta".to_vec(), SequenceNumber::new(2)?, limits)?,
            TableEntry::value(
                b"gamma".to_vec(),
                SequenceNumber::new(1)?,
                b"three".to_vec(),
                limits,
            )?,
        ];
        entries.sort_by(|left, right| left.internal_key.cmp(&right.internal_key));
        Ok(entries)
    }

    #[test]
    fn build_open_and_point_lookup() -> Result<()> {
        let directory = temporary_directory("lookup")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let meta = build_table(
            &filesystem,
            TableBuildRequest::new(
                &directory,
                &directory.join("TEMP"),
                database_id,
                7,
                &entries(&limits)?,
                &limits,
                TableBuildOptions {
                    data_block_size: 1024,
                    restart_interval: 2,
                    bloom_bits_per_key: 10,
                },
            ),
        )?;
        let reader = TableReader::open(
            &filesystem,
            &directory,
            database_id,
            &meta,
            &limits,
            16 * 1024 * 1024,
        )?;
        assert_eq!(reader.get(b"alpha")?, TableLookup::Value(b"one".to_vec()));
        assert_eq!(reader.get(b"beta")?, TableLookup::Tombstone);
        assert_eq!(reader.get(b"missing")?, TableLookup::Missing);
        drop(reader);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn snapshot_lookup_across_multiple_data_blocks_uses_cache() -> Result<()> {
        let directory = temporary_directory("snapshot-block-split")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let mut rows = Vec::new();
        for sequence in (3..=8_u64).rev() {
            rows.push(TableEntry::value(
                b"account".to_vec(),
                SequenceNumber::new(sequence)?,
                vec![u8::try_from(sequence).unwrap_or_default(); 700],
                &limits,
            )?);
        }
        rows.push(TableEntry::tombstone(
            b"account".to_vec(),
            SequenceNumber::new(2)?,
            &limits,
        )?);
        rows.push(TableEntry::value(
            b"account".to_vec(),
            SequenceNumber::new(1)?,
            b"old".to_vec(),
            &limits,
        )?);
        rows.push(TableEntry::value(
            b"other".to_vec(),
            SequenceNumber::new(1)?,
            b"other".to_vec(),
            &limits,
        )?);
        rows.sort_by(|a, b| a.internal_key.cmp(&b.internal_key));
        let meta = build_table(
            &filesystem,
            TableBuildRequest::new(
                &directory,
                &directory.join("TEMP"),
                database_id,
                29,
                &rows,
                &limits,
                TableBuildOptions {
                    data_block_size: 1024,
                    restart_interval: 2,
                    bloom_bits_per_key: 10,
                },
            ),
        )?;
        let reader = TableReader::open(
            &filesystem,
            &directory,
            database_id,
            &meta,
            &limits,
            16 * 1024 * 1024,
        )?;
        assert!(reader.index.len() >= 2);
        let cache = BlockCache::new(16 * 1024);
        assert_eq!(
            reader.get_at_cached(b"account", Some(SequenceNumber::new(4)?), &cache)?,
            TableLookup::Value(vec![4; 700])
        );
        assert_eq!(
            reader.get_at_cached(b"account", Some(SequenceNumber::new(2)?), &cache)?,
            TableLookup::Tombstone
        );
        assert_eq!(
            reader.get_at_cached(b"account", Some(SequenceNumber::new(1)?), &cache)?,
            TableLookup::Value(b"old".to_vec())
        );
        assert_eq!(
            reader.get_at_cached(b"account", None, &cache)?,
            TableLookup::Missing
        );
        assert_eq!(
            reader.latest_sequence_after_cached(
                b"account",
                Some(SequenceNumber::new(7)?),
                &cache
            )?,
            Some(SequenceNumber::new(8)?)
        );
        assert_eq!(
            reader.latest_sequence_after_cached(
                b"account",
                Some(SequenceNumber::new(8)?),
                &cache
            )?,
            None
        );
        assert!(cache.stats()?.hits > 0);
        drop(reader);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn checksum_corruption_is_rejected() -> Result<()> {
        let directory = temporary_directory("checksum")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let meta = build_table(
            &filesystem,
            TableBuildRequest::new(
                &directory,
                &directory.join("TEMP"),
                database_id,
                8,
                &entries(&limits)?,
                &limits,
                TableBuildOptions {
                    data_block_size: 1024,
                    restart_interval: 2,
                    bloom_bits_per_key: 10,
                },
            ),
        )?;
        let path = directory.join(sst_file_name(8));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| Error::io("open_test_sstable", Some(path.clone()), source))?;
        let checksum_offset = u64::try_from(SST_HEADER_SIZE - 4).unwrap_or(u64::MAX);
        file.seek(SeekFrom::Start(checksum_offset))
            .map_err(|source| Error::io("seek_test_sstable", Some(path.clone()), source))?;
        let mut checksum_byte = [0_u8; 1];
        file.read_exact(&mut checksum_byte)
            .map_err(|source| Error::io("read_test_sstable", Some(path.clone()), source))?;
        checksum_byte[0] ^= 0xff;
        file.seek(SeekFrom::Start(checksum_offset))
            .map_err(|source| Error::io("seek_test_sstable", Some(path.clone()), source))?;
        file.write_all(&checksum_byte)
            .map_err(|source| Error::io("corrupt_test_sstable", Some(path.clone()), source))?;
        file.sync_all()
            .map_err(|source| Error::io("sync_test_sstable", Some(path.clone()), source))?;
        drop(file);
        assert!(matches!(
            TableReader::open(
                &filesystem,
                &directory,
                database_id,
                &meta,
                &limits,
                16 * 1024 * 1024,
            ),
            Err(Error::Corruption { .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn future_header_version_is_rejected() -> Result<()> {
        let directory = temporary_directory("future")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let meta = build_table(
            &filesystem,
            TableBuildRequest::new(
                &directory,
                &directory.join("TEMP"),
                database_id,
                9,
                &entries(&limits)?,
                &limits,
                TableBuildOptions {
                    data_block_size: 1024,
                    restart_interval: 2,
                    bloom_bits_per_key: 10,
                },
            ),
        )?;
        let path = directory.join(sst_file_name(9));
        let mut bytes = fs::read(&path)
            .map_err(|source| Error::io("read_test_sstable", Some(path.clone()), source))?;
        bytes[8..10].copy_from_slice(&2_u16.to_le_bytes());
        let checksum = crc32c(&bytes[..60]);
        bytes[60..64].copy_from_slice(&checksum.to_le_bytes());
        fs::write(&path, bytes)
            .map_err(|source| Error::io("write_test_sstable", Some(path.clone()), source))?;
        assert!(matches!(
            TableReader::open(
                &filesystem,
                &directory,
                database_id,
                &meta,
                &limits,
                16 * 1024 * 1024,
            ),
            Err(Error::UnsupportedFormat { .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
