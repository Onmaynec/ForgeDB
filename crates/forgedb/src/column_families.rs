//! Изолированные именованные Column Families поверх проверенного локального LSM path.
//!
//! Каждая family — отдельный WAL/LSM каталог. Реестр хранит только ID↔name;
//! глобальных транзакций между families нет. Старые default файлы не мигрируются.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::ops::RangeBounds;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use forgedb_fs::{FileSystem, NativeFileSystem};

use crate::{
    ColumnFamilyId, Database, DbIterator, Error, Options, ReadOptions, Result, Snapshot,
    TableHandleStats, Transaction, WriteBatch, WriteOptions,
};

const REGISTRY_DIRECTORY: &str = ".forgedb-column-families";
const REGISTRY_NAME: &str = "REGISTRY";
const REGISTRY_TEMP: &str = "REGISTRY.tmp";
const REGISTRY_MAGIC: &[u8; 8] = b"FGCFREG1";
const REGISTRY_VERSION: u32 = 1;
const MAX_FAMILIES: usize = 1024;
const MAX_NAME_BYTES: usize = 64;
const MAX_REGISTRY_BYTES: u64 = 128 * 1024;

/// Идентификатор и имя действующей Column Family.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnFamilyInfo {
    pub id: ColumnFamilyId,
    pub name: String,
}

#[derive(Clone, Debug)]
struct RegistryData {
    next_id: u32,
    names: BTreeMap<u32, String>,
}

impl Default for RegistryData {
    fn default() -> Self {
        Self {
            next_id: 1,
            names: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Default)]
struct RegistryState {
    data: RegistryData,
    opened: BTreeMap<u32, Arc<Database>>,
}

/// Группа независимых LSM families с долговечным реестром имён.
///
/// У каждой family собственные WAL, MANIFEST, sequence numbers и process lock.
/// Транзакции и WriteBatch атомарны только в пределах одной family.
#[derive(Debug)]
pub struct ColumnFamilies {
    root: PathBuf,
    default: Arc<Database>,
    options: Options,
    registry: Mutex<RegistryState>,
    closed: AtomicBool,
}

impl ColumnFamilies {
    /// Открывает default базу без миграции и загружает реестр именованных families.
    pub fn open(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        let default = Arc::new(Database::open(path, options.clone())?);
        let root = default.path().to_path_buf();
        let data = load_registry(&root)?;
        Ok(Self {
            root,
            default,
            options,
            registry: Mutex::new(RegistryState {
                data,
                opened: BTreeMap::new(),
            }),
            closed: AtomicBool::new(false),
        })
    }

    /// Доступ к старому default Database API без изменения существующего каталога.
    #[must_use]
    pub fn default_database(&self) -> &Database {
        &self.default
    }

    /// Возвращает зарегистрированные families, включая default (ID 0).
    pub fn list(&self) -> Result<Vec<ColumnFamilyInfo>> {
        self.ensure_open()?;
        let state = self.lock_registry()?;
        let mut entries = vec![ColumnFamilyInfo {
            id: ColumnFamilyId::DEFAULT,
            name: "default".to_owned(),
        }];
        for (&id, name) in &state.data.names {
            entries.push(ColumnFamilyInfo {
                id: ColumnFamilyId::new(id),
                name: name.clone(),
            });
        }
        Ok(entries)
    }

    /// Создаёт family с уникальным именем и невозобновляемым числовым ID.
    ///
    /// До публикации registry сначала полностью инициализируется дочерняя база.
    /// Каталог от прерванного создания остаётся orphan и никогда не переиспользуется.
    pub fn create(&self, name: &str) -> Result<ColumnFamilyId> {
        self.ensure_writable()?;
        validate_name(name)?;
        let mut state = self.lock_registry()?;
        if name == "default" || state.data.names.values().any(|value| value == name) {
            return Err(Error::invalid_argument(
                "column_family_name",
                "family с этим именем уже существует",
            ));
        }
        if state.data.names.len() >= MAX_FAMILIES {
            return Err(Error::ResourceLimit {
                resource: "column_families",
                limit: u64::try_from(MAX_FAMILIES).unwrap_or(u64::MAX),
                observed: u64::try_from(state.data.names.len().saturating_add(1))
                    .unwrap_or(u64::MAX),
            });
        }

        // Если процесс упал до записи registry, существующий orphan-каталог
        // никогда не должен стать новой зарегистрированной family.
        let mut candidate = state.data.next_id;
        loop {
            let next = candidate.checked_add(1).ok_or(Error::ResourceLimit {
                resource: "column_family_id",
                limit: u64::from(u32::MAX - 1),
                observed: u64::from(candidate),
            })?;
            if !entry_exists(&family_path(&self.root, candidate))? {
                let mut proposed = state.data.clone();
                proposed.next_id = next;
                proposed.names.insert(candidate, name.to_owned());

                let mut child_options = self.options.clone();
                child_options.read_only = false;
                child_options.error_if_exists = false;
                child_options.create_if_missing = true;
                let child = Database::open(family_path(&self.root, candidate), child_options)?;
                child.close()?;
                drop(child);
                persist_registry(&self.root, &proposed)?;
                state.data = proposed;
                return Ok(ColumnFamilyId::new(candidate));
            }
            candidate = next;
        }
    }

    /// Логически удаляет family. Её каталог сохраняется как orphan для безопасной
    /// диагностики, автоматической очистки без подтверждения происхождения нет.
    ///
    /// Физические ID никогда не переиспользуются. Default family удалить нельзя.
    pub fn drop_family(&self, id: ColumnFamilyId) -> Result<()> {
        self.ensure_writable()?;
        if id == ColumnFamilyId::DEFAULT {
            return Err(Error::invalid_argument(
                "column_family",
                "default family не может быть удалена",
            ));
        }
        let mut state = self.lock_registry()?;
        if !state.data.names.contains_key(&id.get()) {
            return Err(unknown_family(id));
        }
        // Sync/close precedes registry removal. Failure keeps the family active.
        if let Some(child) = state.opened.get(&id.get()) {
            child.close()?;
        }
        state.opened.remove(&id.get());
        let mut proposed = state.data.clone();
        proposed.names.remove(&id.get());
        persist_registry(&self.root, &proposed)?;
        state.data = proposed;
        Ok(())
    }

    /// Записывает ключ в выбранную family.
    pub fn put(
        &self,
        id: ColumnFamilyId,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        options: WriteOptions,
    ) -> Result<()> {
        self.resolve(id)?.put(key, value, options)
    }

    /// Добавляет tombstone в выбранную family.
    pub fn delete(
        &self,
        id: ColumnFamilyId,
        key: impl AsRef<[u8]>,
        options: WriteOptions,
    ) -> Result<()> {
        self.resolve(id)?.delete(key, options)
    }

    /// Атомарно применяет обычный WriteBatch внутри одной family.
    ///
    /// Для mixed-family batch нет общего WAL boundary: каждая family имеет
    /// собственный WAL. `WriteBatch::put_cf` здесь намеренно не используется.
    pub fn write(
        &self,
        id: ColumnFamilyId,
        batch: WriteBatch,
        options: WriteOptions,
    ) -> Result<()> {
        self.resolve(id)?.write(batch, options)
    }

    /// Возвращает последнее значение ключа выбранной family.
    pub fn get(&self, id: ColumnFamilyId, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        self.resolve(id)?.get(key)
    }

    /// Snapshot-aware point lookup внутри одной family.
    pub fn get_with_options(
        &self,
        id: ColumnFamilyId,
        key: impl AsRef<[u8]>,
        options: ReadOptions,
    ) -> Result<Option<Vec<u8>>> {
        self.resolve(id)?.get_with_options(key, options)
    }

    /// Snapshot конкретной family, неприменимый к другой family.
    pub fn snapshot(&self, id: ColumnFamilyId) -> Result<Snapshot> {
        self.resolve(id)?.snapshot()
    }

    /// Optimistic snapshot-isolation transaction в пределах одной family.
    pub fn begin_transaction(&self, id: ColumnFamilyId) -> Result<Transaction> {
        self.resolve(id)?.begin_transaction()
    }

    /// Forward scan одной family, не объединяет unrelated keyspaces.
    pub fn scan<R>(&self, id: ColumnFamilyId, range: R, options: ReadOptions) -> Result<DbIterator>
    where
        R: RangeBounds<Vec<u8>>,
    {
        self.resolve(id)?.scan(range, options)
    }

    /// Flush только выбранной family.
    pub fn flush(&self, id: ColumnFamilyId) -> Result<()> {
        self.resolve(id)?.flush()
    }

    /// Метрики point-read SST handles конкретной family, не глобальная FD квота.
    pub fn table_handle_stats(&self, id: ColumnFamilyId) -> Result<TableHandleStats> {
        self.resolve(id)?.table_handle_stats()
    }

    /// Leveled compaction выбранной family.
    pub fn compact_range(
        &self,
        id: ColumnFamilyId,
        start: Option<&[u8]>,
        end: Option<&[u8]>,
    ) -> Result<()> {
        self.resolve(id)?.compact_range(start, end)
    }

    /// Синхронизирует все открытые families перед освобождением корневого lock.
    /// При первой ошибке сохраняет состояние для retry.
    pub fn close(&self) -> Result<()> {
        // The registry mutex serializes close with create/drop/open.
        // Re-check while holding the lock so concurrent close remains idempotent.
        let state = self.registry.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-CF-REGISTRY-MUTEX",
            reason: "column family registry mutex poisoned".to_owned(),
        })?;
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        for child in state.opened.values() {
            child.close()?;
        }
        self.default.close()?;
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn resolve(&self, id: ColumnFamilyId) -> Result<Arc<Database>> {
        self.ensure_open()?;
        if id == ColumnFamilyId::DEFAULT {
            return Ok(Arc::clone(&self.default));
        }
        let mut state = self.lock_registry()?;
        if !state.data.names.contains_key(&id.get()) {
            return Err(unknown_family(id));
        }
        if let Some(database) = state.opened.get(&id.get()) {
            return Ok(Arc::clone(database));
        }
        let path = family_path(&self.root, id.get());
        reject_symlink(&path)?;
        if !path.is_dir() || !path.join("CURRENT").is_file() || !path.join("IDENTITY").is_file() {
            return Err(Error::corruption(
                "column family",
                format!(
                    "зарегистрированная family {} отсутствует или не инициализирована",
                    id.get()
                ),
            ));
        }
        let mut options = self.options.clone();
        options.create_if_missing = false;
        options.error_if_exists = false;
        let database = Arc::new(Database::open(&path, options)?);
        state.opened.insert(id.get(), Arc::clone(&database));
        Ok(database)
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        Ok(())
    }

    fn ensure_writable(&self) -> Result<()> {
        self.ensure_open()?;
        if self.options.read_only {
            return Err(Error::ReadOnly);
        }
        Ok(())
    }

    fn lock_registry(&self) -> Result<MutexGuard<'_, RegistryState>> {
        let state = self.registry.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-CF-REGISTRY-MUTEX",
            reason: "column family registry mutex poisoned".to_owned(),
        })?;
        // An operation may have passed ensure_open before concurrent close
        // obtained the mutex. Do not allow registry mutations after close.
        self.ensure_open()?;
        Ok(state)
    }
}

fn unknown_family(id: ColumnFamilyId) -> Error {
    Error::invalid_argument(
        "column_family",
        format!("family id {} не зарегистрирована", id.get()),
    )
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::invalid_argument(
            "column_family_name",
            "имя должно содержать 1–64 ASCII букв/цифр, '-' или '_'",
        ));
    }
    Ok(())
}

fn registry_dir(root: &Path) -> PathBuf {
    root.join(REGISTRY_DIRECTORY)
}

fn family_path(root: &Path, id: u32) -> PathBuf {
    registry_dir(root).join(format!("CF-{id:010}"))
}

fn entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::io("stat_cf_path", Some(path.to_path_buf()), source)),
    }
}

/// Подмена зарезервированных путей symlink не должна переносить storage I/O
/// за пределы каталога базы. Даже dangling symlink считается занятым путём.
fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(Error::corruption(
            "column family path",
            format!("символьная ссылка недопустима: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::io("stat_cf_path", Some(path.to_path_buf()), source)),
    }
}

fn load_registry(root: &Path) -> Result<RegistryData> {
    reject_symlink(&registry_dir(root))?;
    let path = registry_dir(root).join(REGISTRY_NAME);
    reject_symlink(&path)?;
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RegistryData::default());
        }
        Err(source) => return Err(Error::io("stat_cf_registry", Some(path), source)),
    };
    if metadata.len() > MAX_REGISTRY_BYTES {
        return Err(Error::ResourceLimit {
            resource: "column_family_registry",
            limit: MAX_REGISTRY_BYTES,
            observed: metadata.len(),
        });
    }
    let mut file = File::open(&path)
        .map_err(|source| Error::io("open_cf_registry", Some(path.clone()), source))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| Error::io("read_cf_registry", Some(path), source))?;
    decode_registry(&bytes)
}

fn persist_registry(root: &Path, data: &RegistryData) -> Result<()> {
    let directory = registry_dir(root);
    reject_symlink(&directory)?;
    fs::create_dir_all(&directory)
        .map_err(|source| Error::io("create_cf_directory", Some(directory.clone()), source))?;
    let path = directory.join(REGISTRY_TEMP);
    reject_symlink(&path)?;
    reject_symlink(&directory.join(REGISTRY_NAME))?;
    let bytes = encode_registry(data)?;
    let mut file = File::create(&path)
        .map_err(|source| Error::io("create_cf_registry", Some(path.clone()), source))?;
    file.write_all(&bytes)
        .map_err(|source| Error::io("write_cf_registry", Some(path.clone()), source))?;
    file.sync_all()
        .map_err(|source| Error::io("sync_cf_registry", Some(path.clone()), source))?;
    drop(file);
    let fs = NativeFileSystem;
    fs.rename_atomic(&path, &directory.join(REGISTRY_NAME))?;
    fs.sync_directory(&directory)?;
    fs.sync_directory(root)
}

fn encode_registry(data: &RegistryData) -> Result<Vec<u8>> {
    if data.names.len() > MAX_FAMILIES {
        return Err(Error::corruption("CF registry", "слишком много families"));
    }
    let mut bytes = Vec::with_capacity(256);
    bytes.extend_from_slice(REGISTRY_MAGIC);
    bytes.extend_from_slice(&REGISTRY_VERSION.to_le_bytes());
    bytes.extend_from_slice(&data.next_id.to_le_bytes());
    let count = u32::try_from(data.names.len())
        .map_err(|_| Error::corruption("CF registry", "число families переполнено"))?;
    bytes.extend_from_slice(&count.to_le_bytes());
    for (&id, name) in &data.names {
        validate_name(name)?;
        let len = u8::try_from(name.len())
            .map_err(|_| Error::corruption("CF registry", "имя слишком длинное"))?;
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.push(len);
        bytes.extend_from_slice(name.as_bytes());
    }
    let checksum = crc32c(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_REGISTRY_BYTES {
        return Err(Error::ResourceLimit {
            resource: "column_family_registry",
            limit: MAX_REGISTRY_BYTES,
            observed: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    Ok(bytes)
}

fn decode_registry(bytes: &[u8]) -> Result<RegistryData> {
    const HEADER: usize = 8 + 4 + 4 + 4;
    if bytes.len() < HEADER + 4 || bytes.get(..8) != Some(REGISTRY_MAGIC.as_slice()) {
        return Err(Error::corruption("CF registry", "неверный header"));
    }
    let payload_end = bytes.len() - 4;
    let stored = u32::from_le_bytes(
        bytes[payload_end..]
            .try_into()
            .map_err(|_| Error::corruption("CF registry", "усечён checksum"))?,
    );
    if crc32c(&bytes[..payload_end]) != stored {
        return Err(Error::corruption("CF registry", "CRC32C mismatch"));
    }
    let version = read_u32(bytes, 8)?;
    if version != REGISTRY_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "CF registry",
            found: version,
            minimum_supported: REGISTRY_VERSION,
            maximum_supported: REGISTRY_VERSION,
        });
    }
    let next_id = read_u32(bytes, 12)?;
    let count = usize::try_from(read_u32(bytes, 16)?)
        .map_err(|_| Error::corruption("CF registry", "некорректное число записей"))?;
    if next_id == 0 || count > MAX_FAMILIES {
        return Err(Error::corruption("CF registry", "неверный next_id/count"));
    }
    let mut offset = HEADER;
    let mut names = BTreeMap::new();
    let mut seen_names = BTreeSet::new();
    for _ in 0..count {
        let id = read_u32(bytes, offset)?;
        offset = offset
            .checked_add(4)
            .ok_or_else(|| Error::corruption("CF registry", "offset overflow"))?;
        let len = usize::from(
            *bytes
                .get(offset)
                .ok_or_else(|| Error::corruption("CF registry", "усечён name length"))?,
        );
        offset += 1;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::corruption("CF registry", "name offset overflow"))?;
        if end > payload_end {
            return Err(Error::corruption("CF registry", "усечено имя"));
        }
        let name = std::str::from_utf8(&bytes[offset..end])
            .map_err(|_| Error::corruption("CF registry", "имя не ASCII/UTF-8"))?;
        validate_name(name).map_err(|_| Error::corruption("CF registry", "недопустимое имя"))?;
        if id == 0
            || id >= next_id
            || name == "default"
            || names.insert(id, name.to_owned()).is_some()
            || !seen_names.insert(name.to_owned())
        {
            return Err(Error::corruption(
                "CF registry",
                "повтор/неверный family ID или name",
            ));
        }
        offset = end;
    }
    if offset != payload_end {
        return Err(Error::corruption(
            "CF registry",
            "лишние bytes после records",
        ));
    }
    Ok(RegistryData { next_id, names })
}

fn read_u32(bytes: &[u8], start: usize) -> Result<u32> {
    let end = start
        .checked_add(4)
        .ok_or_else(|| Error::corruption("CF registry", "offset overflow"))?;
    let raw: [u8; 4] = bytes
        .get(start..end)
        .ok_or_else(|| Error::corruption("CF registry", "усечено u32"))?
        .try_into()
        .map_err(|_| Error::corruption("CF registry", "неверный u32"))?;
    Ok(u32::from_le_bytes(raw))
}

fn crc32c(bytes: &[u8]) -> u32 {
    let mut state = !0_u32;
    for byte in bytes {
        state ^= u32::from(*byte);
        for _ in 0..8 {
            state = if state & 1 == 0 {
                state >> 1
            } else {
                (state >> 1) ^ 0x82f6_3b78
            };
        }
    }
    !state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_rejects_damage_and_duplicate_names() -> Result<()> {
        let mut data = RegistryData {
            next_id: 3,
            ..RegistryData::default()
        };
        data.names.insert(1, "events".to_owned());
        data.names.insert(2, "sessions".to_owned());
        let bytes = encode_registry(&data)?;
        let restored = decode_registry(&bytes)?;
        assert_eq!(restored.next_id, 3);
        assert_eq!(restored.names, data.names);
        let mut damaged = bytes.clone();
        damaged[20] ^= 0x80;
        assert!(matches!(
            decode_registry(&damaged),
            Err(Error::Corruption { .. })
        ));
        let mut future = bytes.clone();
        future[8..12].copy_from_slice(&2_u32.to_le_bytes());
        let end = future.len() - 4;
        let checksum = crc32c(&future[..end]);
        future[end..].copy_from_slice(&checksum.to_le_bytes());
        assert!(matches!(
            decode_registry(&future),
            Err(Error::UnsupportedFormat { .. })
        ));
        assert!(validate_name("../escape").is_err());
        assert!(validate_name("тест").is_err());
        Ok(())
    }
}
