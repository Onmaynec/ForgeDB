use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use forgedb_core::{DatabaseId, Error, Result, SequenceNumber, WriteBatch};
use forgedb_fs::{
    FileSystem, IDENTITY_FILE_NAME, NativeFileSystem, NativeLockGuard, load_or_create_identity,
    read_identity,
};
use forgedb_wal::{WAL_FILE_NAME, WalBatch, WalWriter, open_wal, replay_wal};

use crate::memtable::MemTable;
use crate::{Durability, Options, WriteOptions};

const LOCK_FILE_NAME: &str = "LOCK";

#[derive(Debug)]
struct DatabaseState {
    memtable: MemTable,
    wal: Option<WalWriter<File>>,
    next_sequence: SequenceNumber,
    fatal_write_error: Option<String>,
    lock: Option<NativeLockGuard>,
}

/// Встраиваемая локальная key-value база ForgeDB.
#[derive(Debug)]
pub struct Database {
    directory: PathBuf,
    identity: DatabaseId,
    options: Options,
    state: Mutex<DatabaseState>,
    closed: AtomicBool,
}

impl Database {
    /// Открывает базу, проверяет identity и WAL, затем воспроизводит все полные кадры.
    pub fn open(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        options.validate()?;
        let directory = path.as_ref().to_path_buf();
        let existed = directory.exists();
        let identity_path = directory.join(IDENTITY_FILE_NAME);
        let identity_existed = identity_path.exists();

        if !existed && !options.create_if_missing {
            return Err(Error::invalid_argument(
                "path",
                "каталог базы не существует и create_if_missing=false",
            ));
        }
        if options.error_if_exists && identity_existed {
            return Err(Error::invalid_argument(
                "path",
                "база уже существует и error_if_exists=true",
            ));
        }
        if options.read_only && !identity_existed {
            return Err(Error::invalid_argument(
                "read_only",
                "read-only открытие не создаёт новую базу",
            ));
        }

        let filesystem = NativeFileSystem;
        filesystem.create_dir_all(&directory)?;
        let lock = filesystem.lock_exclusive(&directory.join(LOCK_FILE_NAME))?;
        let identity = if options.read_only {
            read_identity(&filesystem, &directory)?
        } else {
            load_or_create_identity(&filesystem, &directory)?
        };
        let limits = options.limits()?;
        let mut memtable = MemTable::default();

        let (wal, last_sequence) = if options.read_only {
            let wal_path = directory.join(WAL_FILE_NAME);
            if !wal_path.exists() {
                return Err(Error::invalid_argument(
                    "read_only",
                    "WAL отсутствует; read-only открытие не создаёт файлы",
                ));
            }
            let summary = replay_wal(&filesystem, &wal_path, identity, &limits, |wal_batch| {
                apply_replayed_batch(&mut memtable, wal_batch, options.memtable_size)
            })?;
            (None, summary.last_sequence)
        } else {
            let (writer, summary) =
                open_wal(&filesystem, &directory, identity, &limits, |wal_batch| {
                    apply_replayed_batch(&mut memtable, wal_batch, options.memtable_size)
                })?;
            (Some(writer), summary.last_sequence)
        };
        let next_sequence = match last_sequence {
            Some(last) => last.checked_next()?,
            None => SequenceNumber::FIRST,
        };

        Ok(Self {
            directory,
            identity,
            options,
            state: Mutex::new(DatabaseState {
                memtable,
                wal,
                next_sequence,
                fatal_write_error: None,
                lock: Some(lock),
            }),
            closed: AtomicBool::new(false),
        })
    }

    /// Записывает или заменяет значение одного ключа.
    pub fn put(
        &self,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        write_options: WriteOptions,
    ) -> Result<()> {
        let mut batch = WriteBatch::with_limits(self.options.limits()?);
        batch.put(key, value)?;
        self.write(batch, write_options)
    }

    /// Добавляет tombstone для одного ключа.
    pub fn delete(&self, key: impl AsRef<[u8]>, write_options: WriteOptions) -> Result<()> {
        let mut batch = WriteBatch::with_limits(self.options.limits()?);
        batch.delete(key)?;
        self.write(batch, write_options)
    }

    /// Применяет WriteBatch атомарно относительно читателей и recovery.
    pub fn write(&self, batch: WriteBatch, write_options: WriteOptions) -> Result<()> {
        self.ensure_open()?;
        if self.options.read_only {
            return Err(Error::ReadOnly);
        }
        let limits = self.options.limits()?;
        batch.validate_against(&limits)?;
        if batch.is_empty() {
            return Err(Error::invalid_argument(
                "batch",
                "batch не должен быть пустым",
            ));
        }

        let mut state = self.lock_state()?;
        if let Some(reason) = &state.fatal_write_error {
            return Err(Error::BackgroundFailure {
                component: "WAL writer",
                reason: reason.clone(),
            });
        }
        let sequence_start = state.next_sequence;
        let sequence_end = sequence_start.inclusive_end(batch.len())?;
        let next_sequence = sequence_end.checked_next()?;
        let prepared =
            state
                .memtable
                .prepare(sequence_start, &batch, self.options.memtable_size)?;

        let append_result = match write_options.durability() {
            Durability::Sync => state
                .wal
                .as_mut()
                .ok_or(Error::InternalInvariant {
                    invariant: "I-WAL-WRITER-PRESENT",
                    reason: "write path не содержит WAL writer".to_owned(),
                })?
                .append_batch(sequence_start, &batch, &limits, true)
                .map(|_| ()),
            Durability::NoSync => state
                .wal
                .as_mut()
                .ok_or(Error::InternalInvariant {
                    invariant: "I-WAL-WRITER-PRESENT",
                    reason: "write path не содержит WAL writer".to_owned(),
                })?
                .append_batch(sequence_start, &batch, &limits, false)
                .map(|_| ()),
            Durability::DisableWal => Ok(()),
        };
        if let Err(error) = append_result {
            state.fatal_write_error = Some(error.to_string());
            return Err(error);
        }

        state.memtable.apply(prepared);
        state.next_sequence = next_sequence;
        Ok(())
    }

    /// Возвращает последнее видимое значение или `None` для отсутствующего ключа/tombstone.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        self.ensure_open()?;
        let key = key.as_ref();
        self.options.limits()?.validate_key(key)?;
        let state = self.lock_state()?;
        Ok(state.memtable.get(key))
    }

    /// Синхронизирует WAL и закрывает базу для последующих вызовов API.
    pub fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let mut state = self.lock_state_allow_closed()?;
        if let Some(writer) = state.wal.take() {
            writer.sync()?;
        }
        state.lock.take();
        Ok(())
    }

    /// Возвращает стабильный идентификатор базы.
    #[must_use]
    pub const fn identity(&self) -> DatabaseId {
        self.identity
    }

    /// Возвращает путь открытого каталога.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.directory
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, DatabaseState>> {
        self.ensure_open()?;
        self.lock_state_allow_closed()
    }

    fn lock_state_allow_closed(&self) -> Result<MutexGuard<'_, DatabaseState>> {
        self.state.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-DATABASE-MUTEX",
            reason: "mutex состояния базы poisoned".to_owned(),
        })
    }
}

fn apply_replayed_batch(
    memtable: &mut MemTable,
    wal_batch: WalBatch,
    memory_limit: usize,
) -> Result<()> {
    let prepared = memtable.prepare(wal_batch.sequence_start, &wal_batch.batch, memory_limit)?;
    memtable.apply(prepared);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("forgedb-db-{name}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn sync_put_delete_and_batch_survive_reopen() -> Result<()> {
        let directory = temporary_directory("reopen")?;
        let db = Database::open(&directory, Options::default())?;
        db.put(b"a", b"one", WriteOptions::sync())?;
        let mut batch = WriteBatch::new();
        batch.put(b"b", b"two")?;
        batch.delete(b"a")?;
        db.write(batch, WriteOptions::sync())?;
        db.close()?;

        let reopened = Database::open(&directory, Options::default())?;
        assert_eq!(reopened.get(b"a")?, None);
        assert_eq!(reopened.get(b"b")?, Some(b"two".to_vec()));
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn deterministic_model_matches_after_reopen() -> Result<()> {
        let directory = temporary_directory("model")?;
        let db = Database::open(&directory, Options::default())?;
        let mut model = BTreeMap::<Vec<u8>, Option<Vec<u8>>>::new();
        let mut state = 0x4d59_5df4_d0f3_3173_u64;
        for step in 0..500_u64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let key = format!("key-{}", state % 37).into_bytes();
            if state & 3 == 0 {
                db.delete(&key, WriteOptions::sync())?;
                model.insert(key, None);
            } else {
                let value = format!("value-{step}-{}", state >> 16).into_bytes();
                db.put(&key, &value, WriteOptions::sync())?;
                model.insert(key, Some(value));
            }
        }
        db.close()?;

        let reopened = Database::open(&directory, Options::default())?;
        for (key, expected) in model {
            assert_eq!(reopened.get(&key)?, expected);
        }
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn read_only_open_replays_but_rejects_writes() -> Result<()> {
        let directory = temporary_directory("readonly")?;
        let db = Database::open(&directory, Options::default())?;
        db.put(b"key", b"value", WriteOptions::sync())?;
        db.close()?;

        let options = Options {
            read_only: true,
            ..Options::default()
        };
        let readonly = Database::open(&directory, options)?;
        assert_eq!(readonly.get(b"key")?, Some(b"value".to_vec()));
        assert!(matches!(
            readonly.put(b"other", b"x", WriteOptions::sync()),
            Err(Error::ReadOnly)
        ));
        readonly.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
