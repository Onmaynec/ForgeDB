use std::collections::BTreeSet;

use forgedb_core::{BatchOperation, Error, Result, WriteBatch};

use crate::database_v3_fixed::StreamingDatabase;
use crate::{ReadOptions, Snapshot, WriteOptions};

/// Состояние optimistic transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionState {
    /// Transaction принимает reads и buffered writes.
    Active,
    /// Atomic WriteBatch установлен обычным WAL write path.
    Committed,
    /// Buffered writes отброшены caller-ом.
    RolledBack,
    /// Commit обнаружил write-write conflict.
    Conflicted,
    /// Commit завершился другой storage/error ошибкой.
    Failed,
}

/// Optimistic transaction с snapshot isolation и buffered own writes.
///
/// Reads видят сначала собственные buffered mutations, затем snapshot базы на момент `BEGIN`.
/// Commit валидирует только write-write conflicts: если любой записываемый ключ получил более
/// новую committed version после transaction snapshot, commit отклоняется целиком.
#[derive(Debug)]
pub struct Transaction {
    database: StreamingDatabase,
    snapshot: Option<Snapshot>,
    read_set: BTreeSet<Vec<u8>>,
    write_batch: WriteBatch,
    state: TransactionState,
    max_tracked_reads: usize,
}

impl Transaction {
    pub(crate) fn begin(database: StreamingDatabase) -> Result<Self> {
        let snapshot = database.snapshot()?;
        let (write_batch, max_tracked_reads) = database.new_transaction_batch()?;
        Ok(Self {
            database,
            snapshot: Some(snapshot),
            read_set: BTreeSet::new(),
            write_batch,
            state: TransactionState::Active,
            max_tracked_reads,
        })
    }

    /// Возвращает текущее terminal/active состояние transaction.
    #[must_use]
    pub const fn state(&self) -> TransactionState {
        self.state
    }

    /// Читает ключ: own writes имеют приоритет над зафиксированным snapshot.
    pub fn get(&mut self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        self.ensure_active()?;
        let key = key.as_ref();
        if let Some(own) = self.own_write(key) {
            return Ok(own);
        }
        if !self.read_set.contains(key) {
            if self.read_set.len() >= self.max_tracked_reads {
                return Err(Error::ResourceLimit {
                    resource: "transaction_read_set",
                    limit: u64::try_from(self.max_tracked_reads).unwrap_or(u64::MAX),
                    observed: u64::try_from(self.read_set.len().saturating_add(1))
                        .unwrap_or(u64::MAX),
                });
            }
            self.read_set.insert(key.to_vec());
        }
        let snapshot = self.snapshot.as_ref().ok_or(Error::InternalInvariant {
            invariant: "I-TRANSACTION-ACTIVE-SNAPSHOT",
            reason: "active transaction не содержит snapshot".to_owned(),
        })?;
        self.database
            .get_with_options(key, ReadOptions::from_snapshot(snapshot))
    }

    /// Буферизует Put. Mutation не становится видимой другим readers до commit.
    pub fn put(&mut self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<()> {
        self.ensure_active()?;
        self.write_batch.put(key, value)
    }

    /// Буферизует Delete tombstone.
    pub fn delete(&mut self, key: impl AsRef<[u8]>) -> Result<()> {
        self.ensure_active()?;
        self.write_batch.delete(key)
    }

    /// Валидирует write set под single-writer serialization и атомарно записывает batch.
    ///
    /// `DisableWal` для transaction commit запрещён: transaction durability должна проходить
    /// через обычный WAL protocol. Доступны `Sync` и `NoSync` с теми же гарантиями, что у
    /// `Database::write`.
    pub fn commit(&mut self, write_options: WriteOptions) -> Result<()> {
        self.ensure_active()?;
        if self.write_batch.is_empty() {
            self.state = TransactionState::Committed;
            self.snapshot.take();
            return Ok(());
        }
        let snapshot = self.snapshot.as_ref().ok_or(Error::InternalInvariant {
            invariant: "I-TRANSACTION-ACTIVE-SNAPSHOT",
            reason: "active transaction не содержит snapshot".to_owned(),
        })?;
        let batch = std::mem::take(&mut self.write_batch);
        let result = self
            .database
            .commit_transaction(snapshot, batch, write_options);
        self.snapshot.take();
        match result {
            Ok(()) => {
                self.state = TransactionState::Committed;
                Ok(())
            }
            Err(error @ Error::Conflict { .. }) => {
                self.state = TransactionState::Conflicted;
                Err(error)
            }
            Err(error) => {
                self.state = TransactionState::Failed;
                Err(error)
            }
        }
    }

    /// Отбрасывает buffered writes. На disk ничего не записывается.
    pub fn rollback(&mut self) -> Result<()> {
        self.ensure_active()?;
        self.write_batch = WriteBatch::new();
        self.read_set.clear();
        self.snapshot.take();
        self.state = TransactionState::RolledBack;
        Ok(())
    }

    fn own_write(&self, key: &[u8]) -> Option<Option<Vec<u8>>> {
        self.write_batch
            .operations()
            .iter()
            .rev()
            .find_map(|operation| match operation {
                BatchOperation::Put {
                    key: operation_key,
                    value,
                    ..
                } if operation_key.as_slice() == key => Some(Some(value.clone())),
                BatchOperation::Delete {
                    key: operation_key, ..
                } if operation_key.as_slice() == key => Some(None),
                _ => None,
            })
    }

    fn ensure_active(&self) -> Result<()> {
        if self.state != TransactionState::Active {
            return Err(Error::invalid_argument(
                "transaction",
                format!(
                    "operation требует Active state; текущее состояние: {:?}",
                    self.state
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::{Database, Options, WriteOptions};

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-v050-transaction-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn own_writes_are_visible_before_commit() -> Result<()> {
        let directory = temporary_directory("own-writes")?;
        let database = Database::open(&directory, Options::default())?;
        database.put(b"key", b"base", WriteOptions::sync())?;
        let mut transaction = database.begin_transaction()?;
        transaction.put(b"key", b"updated")?;
        assert_eq!(transaction.get(b"key")?, Some(b"updated".to_vec()));
        transaction.delete(b"key")?;
        assert_eq!(transaction.get(b"key")?, None);
        assert_eq!(database.get(b"key")?, Some(b"base".to_vec()));
        transaction.rollback()?;
        assert_eq!(database.get(b"key")?, Some(b"base".to_vec()));
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn same_key_write_write_conflict_rejects_second_commit() -> Result<()> {
        let directory = temporary_directory("conflict")?;
        let database = Database::open(&directory, Options::default())?;
        database.put(b"key", b"base", WriteOptions::sync())?;
        let mut first = database.begin_transaction()?;
        let mut second = database.begin_transaction()?;
        first.put(b"key", b"first")?;
        second.put(b"key", b"second")?;
        first.commit(WriteOptions::sync())?;
        let result = second.commit(WriteOptions::sync());
        assert!(matches!(result, Err(Error::Conflict { .. })));
        assert_eq!(second.state(), TransactionState::Conflicted);
        assert_eq!(database.get(b"key")?, Some(b"first".to_vec()));
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn sequence_change_conflicts_even_when_value_returns_to_snapshot_bytes() -> Result<()> {
        let directory = temporary_directory("sequence-conflict")?;
        let database = Database::open(&directory, Options::default())?;
        database.put(b"key", b"base", WriteOptions::sync())?;
        let mut transaction = database.begin_transaction()?;
        database.put(b"key", b"temporary", WriteOptions::sync())?;
        database.put(b"key", b"base", WriteOptions::sync())?;
        transaction.put(b"key", b"transaction")?;
        let result = transaction.commit(WriteOptions::sync());
        assert!(matches!(result, Err(Error::Conflict { .. })));
        assert_eq!(database.get(b"key")?, Some(b"base".to_vec()));
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn conflict_is_found_after_newer_version_is_flushed_to_sstable() -> Result<()> {
        let directory = temporary_directory("sst-conflict")?;
        let options = Options {
            l0_compaction_trigger: 8,
            l0_slowdown_trigger: 9,
            l0_stop_trigger: 10,
            ..Options::default()
        };
        let database = Database::open(&directory, options)?;
        database.put(b"key", b"base", WriteOptions::sync())?;
        database.flush()?;
        let mut transaction = database.begin_transaction()?;
        database.put(b"key", b"newer", WriteOptions::sync())?;
        database.flush()?;
        transaction.put(b"key", b"transaction")?;
        let result = transaction.commit(WriteOptions::sync());
        assert!(matches!(result, Err(Error::Conflict { .. })));
        assert_eq!(database.get(b"key")?, Some(b"newer".to_vec()));
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn disjoint_write_sets_can_commit_from_same_snapshot() -> Result<()> {
        let directory = temporary_directory("disjoint")?;
        let database = Database::open(&directory, Options::default())?;
        let mut first = database.begin_transaction()?;
        let mut second = database.begin_transaction()?;
        first.put(b"a", b"one")?;
        second.put(b"b", b"two")?;
        first.commit(WriteOptions::sync())?;
        second.commit(WriteOptions::sync())?;
        assert_eq!(database.get(b"a")?, Some(b"one".to_vec()));
        assert_eq!(database.get(b"b")?, Some(b"two".to_vec()));
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn snapshot_read_stays_stable_while_other_key_can_commit() -> Result<()> {
        let directory = temporary_directory("snapshot")?;
        let database = Database::open(&directory, Options::default())?;
        database.put(b"observed", b"old", WriteOptions::sync())?;
        let mut transaction = database.begin_transaction()?;
        database.put(b"observed", b"new", WriteOptions::sync())?;
        assert_eq!(transaction.get(b"observed")?, Some(b"old".to_vec()));
        transaction.put(b"other", b"value")?;
        transaction.commit(WriteOptions::sync())?;
        assert_eq!(database.get(b"observed")?, Some(b"new".to_vec()));
        assert_eq!(database.get(b"other")?, Some(b"value".to_vec()));
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn disable_wal_is_rejected_for_transaction_commit() -> Result<()> {
        let directory = temporary_directory("disable-wal")?;
        let database = Database::open(&directory, Options::default())?;
        let mut transaction = database.begin_transaction()?;
        transaction.put(b"key", b"value")?;
        let result = transaction.commit(WriteOptions::disable_wal());
        assert!(matches!(result, Err(Error::InvalidArgument { .. })));
        assert_eq!(transaction.state(), TransactionState::Failed);
        assert_eq!(database.get(b"key")?, None);
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn committed_transaction_recovers_as_one_wal_batch() -> Result<()> {
        let directory = temporary_directory("recovery")?;
        let database = Database::open(&directory, Options::default())?;
        let mut transaction = database.begin_transaction()?;
        transaction.put(b"a", b"one")?;
        transaction.put(b"b", b"two")?;
        transaction.commit(WriteOptions::sync())?;
        drop(transaction);
        database.close()?;

        let reopened = Database::open(&directory, Options::default())?;
        assert_eq!(reopened.get(b"a")?, Some(b"one".to_vec()));
        assert_eq!(reopened.get(b"b")?, Some(b"two".to_vec()));
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
