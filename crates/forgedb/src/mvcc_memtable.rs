use std::cmp::Ordering;
use std::collections::BTreeMap;

use forgedb_core::{
    BatchOperation, Error, InternalKey, Limits, Result, SequenceNumber, ValueType, WriteBatch,
};
use forgedb_sstable::TableEntry;

const ENTRY_OVERHEAD_BYTES: usize = 72;

/// Результат snapshot-aware lookup в памяти.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MemLookup {
    Missing,
    Tombstone,
    Value(Vec<u8>),
}

/// Полностью проверенное изменение memtable, применяемое одной критической секцией.
pub(crate) struct PreparedBatch {
    changes: Vec<TableEntry>,
    memory_usage: usize,
}

/// MVCC memtable: user key по возрастанию, sequence и value type по убыванию.
#[derive(Clone, Debug, Default)]
pub(crate) struct MvccMemTable {
    entries: BTreeMap<InternalKey, Vec<u8>>,
    memory_usage: usize,
}

impl MvccMemTable {
    pub(crate) fn get_at(&self, user_key: &[u8], snapshot: Option<SequenceNumber>) -> MemLookup {
        let Some(snapshot) = snapshot else {
            return MemLookup::Missing;
        };
        for (internal_key, value) in &self.entries {
            match internal_key.user_key().cmp(user_key) {
                Ordering::Less => continue,
                Ordering::Greater => break,
                Ordering::Equal if internal_key.sequence() > snapshot => continue,
                Ordering::Equal => {
                    return match internal_key.value_type() {
                        ValueType::Deletion => MemLookup::Tombstone,
                        ValueType::Value => MemLookup::Value(value.clone()),
                    };
                }
            }
        }
        MemLookup::Missing
    }

    /// Возвращает newest committed sequence для user key внутри этой memtable.
    pub(crate) fn latest_sequence(&self, user_key: &[u8]) -> Option<SequenceNumber> {
        for internal_key in self.entries.keys() {
            match internal_key.user_key().cmp(user_key) {
                Ordering::Less => continue,
                Ordering::Greater => break,
                Ordering::Equal => return Some(internal_key.sequence()),
            }
        }
        None
    }

    pub(crate) fn prepare(
        &self,
        sequence_start: SequenceNumber,
        batch: &WriteBatch,
        limits: &Limits,
        memory_limit: usize,
    ) -> Result<PreparedBatch> {
        if batch.is_empty() {
            return Err(Error::invalid_argument(
                "batch",
                "batch не должен быть пустым",
            ));
        }
        sequence_start.inclusive_end(batch.len())?;

        let mut changes = Vec::with_capacity(batch.len());
        let mut projected = self.memory_usage;
        for (index, operation) in batch.operations().iter().enumerate() {
            let offset = u64::try_from(index).map_err(|_| Error::ResourceLimit {
                resource: "sequence_range",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
            let sequence = sequence_start.checked_add(offset)?;
            let entry = match operation {
                BatchOperation::Put { key, value, .. } => {
                    TableEntry::value(key.clone(), sequence, value.clone(), limits)?
                }
                BatchOperation::Delete { key, .. } => {
                    TableEntry::tombstone(key.clone(), sequence, limits)?
                }
            };
            projected = projected
                .checked_add(entry_size(&entry)?)
                .ok_or(Error::ResourceLimit {
                    resource: "memtable_memory",
                    limit: u64::try_from(memory_limit).unwrap_or(u64::MAX),
                    observed: u64::MAX,
                })?;
            if projected > memory_limit {
                return Err(Error::ResourceLimit {
                    resource: "memtable_memory",
                    limit: u64::try_from(memory_limit).unwrap_or(u64::MAX),
                    observed: u64::try_from(projected).unwrap_or(u64::MAX),
                });
            }
            changes.push(entry);
        }

        Ok(PreparedBatch {
            changes,
            memory_usage: projected,
        })
    }

    pub(crate) fn apply(&mut self, prepared: PreparedBatch) {
        for entry in prepared.changes {
            self.entries.insert(entry.internal_key, entry.value);
        }
        self.memory_usage = prepared.memory_usage;
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn memory_usage(&self) -> usize {
        self.memory_usage
    }

    pub(crate) fn table_entries(&self) -> Vec<TableEntry> {
        self.entries
            .iter()
            .map(|(internal_key, value)| TableEntry {
                internal_key: internal_key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

fn entry_size(entry: &TableEntry) -> Result<usize> {
    entry
        .internal_key
        .user_key()
        .len()
        .checked_add(entry.value.len())
        .and_then(|size| size.checked_add(ENTRY_OVERHEAD_BYTES))
        .and_then(|size| size.checked_add(std::mem::size_of::<InternalKey>()))
        .ok_or(Error::ResourceLimit {
            resource: "memtable_entry_size",
            limit: u64::MAX,
            observed: u64::MAX,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reads_older_version() -> Result<()> {
        let limits = Limits::default();
        let mut table = MvccMemTable::default();

        let mut first = WriteBatch::new();
        first.put(b"key", b"old")?;
        let prepared = table.prepare(SequenceNumber::FIRST, &first, &limits, 4096)?;
        table.apply(prepared);

        let mut second = WriteBatch::new();
        second.put(b"key", b"new")?;
        let prepared = table.prepare(SequenceNumber::new(2)?, &second, &limits, 4096)?;
        table.apply(prepared);

        assert_eq!(
            table.get_at(b"key", Some(SequenceNumber::FIRST)),
            MemLookup::Value(b"old".to_vec())
        );
        assert_eq!(
            table.get_at(b"key", Some(SequenceNumber::new(2)?)),
            MemLookup::Value(b"new".to_vec())
        );
        assert_eq!(table.latest_sequence(b"key"), Some(SequenceNumber::new(2)?));
        Ok(())
    }

    #[test]
    fn tombstone_is_snapshot_visible() -> Result<()> {
        let limits = Limits::default();
        let mut table = MvccMemTable::default();
        let mut batch = WriteBatch::new();
        batch.put(b"key", b"value")?;
        batch.delete(b"key")?;
        let prepared = table.prepare(SequenceNumber::FIRST, &batch, &limits, 4096)?;
        table.apply(prepared);
        assert_eq!(
            table.get_at(b"key", Some(SequenceNumber::FIRST)),
            MemLookup::Value(b"value".to_vec())
        );
        assert_eq!(
            table.get_at(b"key", Some(SequenceNumber::new(2)?)),
            MemLookup::Tombstone
        );
        assert_eq!(table.latest_sequence(b"key"), Some(SequenceNumber::new(2)?));
        Ok(())
    }

    #[test]
    fn rejected_batch_preserves_state() -> Result<()> {
        let limits = Limits::default();
        let table = MvccMemTable::default();
        let mut batch = WriteBatch::new();
        batch.put(b"key", b"value")?;
        assert!(
            table
                .prepare(SequenceNumber::FIRST, &batch, &limits, 1)
                .is_err()
        );
        assert_eq!(table.memory_usage(), 0);
        Ok(())
    }
}
