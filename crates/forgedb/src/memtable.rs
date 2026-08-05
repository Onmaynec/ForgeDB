use std::collections::BTreeMap;

#[cfg(test)]
use forgedb_core::ValueType;
use forgedb_core::{BatchOperation, Error, Limits, Result, SequenceNumber, WriteBatch};
use forgedb_sstable::TableEntry;

const ENTRY_OVERHEAD_BYTES: usize = 64;

#[derive(Clone, Debug)]
struct MemEntry {
    sequence: SequenceNumber,
    value: Option<Vec<u8>>,
}

/// Результат point lookup в memtable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MemLookup {
    Missing,
    Tombstone,
    Value(Vec<u8>),
}

/// Подготовленное атомарное изменение memtable.
pub(crate) struct PreparedBatch {
    changes: Vec<(Vec<u8>, MemEntry)>,
    memory_usage: usize,
}

/// Mutable или immutable memtable на базе BTreeMap.
#[derive(Clone, Debug, Default)]
pub(crate) struct MemTable {
    entries: BTreeMap<Vec<u8>, MemEntry>,
    memory_usage: usize,
}

impl MemTable {
    pub(crate) fn get(&self, key: &[u8]) -> MemLookup {
        match self.entries.get(key) {
            None => MemLookup::Missing,
            Some(entry) => match &entry.value {
                Some(value) => MemLookup::Value(value.clone()),
                None => MemLookup::Tombstone,
            },
        }
    }

    pub(crate) fn prepare(
        &self,
        sequence_start: SequenceNumber,
        batch: &WriteBatch,
        memory_limit: usize,
    ) -> Result<PreparedBatch> {
        if batch.is_empty() {
            return Err(Error::invalid_argument(
                "batch",
                "batch не должен быть пустым",
            ));
        }
        sequence_start.inclusive_end(batch.len())?;

        let mut staged = BTreeMap::<Vec<u8>, MemEntry>::new();
        for (index, operation) in batch.operations().iter().enumerate() {
            let offset = u64::try_from(index).map_err(|_| Error::ResourceLimit {
                resource: "sequence_range",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
            let sequence = sequence_start.checked_add(offset)?;
            match operation {
                BatchOperation::Put { key, value, .. } => {
                    staged.insert(
                        key.clone(),
                        MemEntry {
                            sequence,
                            value: Some(value.clone()),
                        },
                    );
                }
                BatchOperation::Delete { key, .. } => {
                    staged.insert(
                        key.clone(),
                        MemEntry {
                            sequence,
                            value: None,
                        },
                    );
                }
            }
        }

        let mut projected = self.memory_usage;
        for (key, entry) in &staged {
            if let Some(existing) = self.entries.get(key) {
                projected = projected.checked_sub(entry_size(key, existing)?).ok_or(
                    Error::InternalInvariant {
                        invariant: "I-MEMTABLE-ACCOUNTING",
                        reason: "memory accounting стал отрицательным".to_owned(),
                    },
                )?;
            }
            projected =
                projected
                    .checked_add(entry_size(key, entry)?)
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
        }

        Ok(PreparedBatch {
            changes: staged.into_iter().collect(),
            memory_usage: projected,
        })
    }

    pub(crate) fn apply(&mut self, prepared: PreparedBatch) {
        for (key, entry) in prepared.changes {
            self.entries.insert(key, entry);
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

    #[must_use]
    pub(crate) fn maximum_sequence(&self) -> Option<SequenceNumber> {
        self.entries.values().map(|entry| entry.sequence).max()
    }

    /// Преобразует одно последнее состояние каждого user key в sorted SSTable entries.
    pub(crate) fn table_entries(&self, limits: &Limits) -> Result<Vec<TableEntry>> {
        let mut entries = Vec::with_capacity(self.entries.len());
        for (key, entry) in &self.entries {
            let table_entry = match &entry.value {
                Some(value) => {
                    TableEntry::value(key.clone(), entry.sequence, value.clone(), limits)?
                }
                None => TableEntry::tombstone(key.clone(), entry.sequence, limits)?,
            };
            entries.push(table_entry);
        }
        entries.sort_by(|left, right| left.internal_key.cmp(&right.internal_key));
        Ok(entries)
    }

    #[cfg(test)]
    pub(crate) fn value_type(&self, key: &[u8]) -> Option<ValueType> {
        self.entries.get(key).map(|entry| {
            if entry.value.is_some() {
                ValueType::Value
            } else {
                ValueType::Deletion
            }
        })
    }
}

fn entry_size(key: &[u8], entry: &MemEntry) -> Result<usize> {
    let value_size = entry.value.as_ref().map_or(0, Vec::len);
    key.len()
        .checked_add(value_size)
        .and_then(|size| size.checked_add(ENTRY_OVERHEAD_BYTES))
        .and_then(|size| size.checked_add(std::mem::size_of_val(&entry.sequence)))
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
    fn rejected_batch_does_not_change_memtable() -> Result<()> {
        let table = MemTable::default();
        let mut batch = WriteBatch::new();
        batch.put(b"key", b"value")?;
        assert!(table.prepare(SequenceNumber::FIRST, &batch, 1).is_err());
        assert_eq!(table.memory_usage(), 0);
        assert_eq!(table.get(b"key"), MemLookup::Missing);
        Ok(())
    }

    #[test]
    fn repeated_key_uses_last_operation() -> Result<()> {
        let mut table = MemTable::default();
        let mut batch = WriteBatch::new();
        batch.put(b"key", b"value")?;
        batch.delete(b"key")?;
        let prepared = table.prepare(SequenceNumber::FIRST, &batch, 1024)?;
        table.apply(prepared);
        assert_eq!(table.get(b"key"), MemLookup::Tombstone);
        assert_eq!(table.value_type(b"key"), Some(ValueType::Deletion));
        Ok(())
    }
}
