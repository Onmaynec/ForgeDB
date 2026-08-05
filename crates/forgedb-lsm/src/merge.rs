use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::fmt::Debug;

use forgedb_core::{ColumnFamilyId, Error, Result, SequenceNumber, ValueType};
use forgedb_sstable::TableEntry;

/// Incremental sorted source для compaction merge.
pub trait CompactionCursor: Debug + Send {
    fn current(&self) -> Option<&TableEntry>;
    fn advance(&mut self) -> Result<()>;
}

/// Владеющий cursor для тестов и уже materialized источников.
#[derive(Debug)]
pub struct VecCompactionCursor {
    entries: Vec<TableEntry>,
    position: usize,
}

impl VecCompactionCursor {
    #[must_use]
    pub fn new(entries: Vec<TableEntry>) -> Self {
        Self {
            entries,
            position: 0,
        }
    }
}

impl CompactionCursor for VecCompactionCursor {
    fn current(&self) -> Option<&TableEntry> {
        self.entries.get(self.position)
    }

    fn advance(&mut self) -> Result<()> {
        if self.position < self.entries.len() {
            self.position = self
                .position
                .checked_add(1)
                .ok_or(Error::ResourceLimit {
                    resource: "compaction_cursor_position",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?;
        }
        Ok(())
    }
}

/// Bounded retention policy одного compaction merge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompactionMergeOptions {
    /// Самый старый active snapshot. `None` означает отсутствие active snapshots.
    pub oldest_snapshot: Option<SequenceNumber>,
    /// Output level является bottommost для выбранного key range.
    pub bottommost: bool,
    /// Максимальное число одновременно объединяемых sorted sources.
    pub max_sources: usize,
    /// Максимальное число versions одного logical key, разрешённое во входах.
    pub max_versions_per_user_key: usize,
}

impl Default for CompactionMergeOptions {
    fn default() -> Self {
        Self {
            oldest_snapshot: None,
            bottommost: false,
            max_sources: 1_024,
            max_versions_per_user_key: 1_000_000,
        }
    }
}

impl CompactionMergeOptions {
    pub fn validate(self) -> Result<()> {
        if self.max_sources == 0 {
            return Err(Error::invalid_argument(
                "max_sources",
                "compaction merge требует минимум один source",
            ));
        }
        if self.max_versions_per_user_key == 0 {
            return Err(Error::invalid_argument(
                "max_versions_per_user_key",
                "лимит versions должен быть больше нуля",
            ));
        }
        Ok(())
    }
}

/// Счётчики одного merge. Они не являются persisted state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompactionMergeStats {
    pub input_entries: u64,
    pub output_entries: u64,
    pub dropped_versions: u64,
    pub dropped_tombstones: u64,
    pub user_keys: u64,
    pub peak_versions_per_user_key: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LogicalKey {
    column_family: ColumnFamilyId,
    user_key: Vec<u8>,
}

impl LogicalKey {
    fn from_entry(entry: &TableEntry) -> Self {
        Self {
            column_family: entry.internal_key.column_family(),
            user_key: entry.internal_key.user_key().to_vec(),
        }
    }

    fn matches(&self, entry: &TableEntry) -> bool {
        self.column_family == entry.internal_key.column_family()
            && self.user_key == entry.internal_key.user_key()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HeapItem {
    entry: TableEntry,
    source_index: usize,
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .entry
            .internal_key
            .cmp(&self.entry.internal_key)
            .then_with(|| other.source_index.cmp(&self.source_index))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Forward bounded-memory merge compaction inputs.
///
/// Объект удерживает один current entry на source и не более
/// `max_versions_per_user_key` entries текущего logical key.
#[derive(Debug)]
pub struct CompactionMerge {
    sources: Vec<Box<dyn CompactionCursor>>,
    heap: BinaryHeap<HeapItem>,
    pending: VecDeque<TableEntry>,
    options: CompactionMergeOptions,
    stats: CompactionMergeStats,
    failed: bool,
}

impl CompactionMerge {
    pub fn new(
        sources: Vec<Box<dyn CompactionCursor>>,
        options: CompactionMergeOptions,
    ) -> Result<Self> {
        options.validate()?;
        if sources.len() > options.max_sources {
            return Err(Error::ResourceLimit {
                resource: "compaction_merge_sources",
                limit: u64::try_from(options.max_sources).unwrap_or(u64::MAX),
                observed: u64::try_from(sources.len()).unwrap_or(u64::MAX),
            });
        }
        let mut merge = Self {
            sources,
            heap: BinaryHeap::new(),
            pending: VecDeque::new(),
            options,
            stats: CompactionMergeStats::default(),
            failed: false,
        };
        for source_index in 0..merge.sources.len() {
            merge.queue_current(source_index)?;
        }
        Ok(merge)
    }

    #[must_use]
    pub const fn stats(&self) -> CompactionMergeStats {
        self.stats
    }

    fn queue_current(&mut self, source_index: usize) -> Result<()> {
        let source = self
            .sources
            .get(source_index)
            .ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-MERGE-SOURCE",
                reason: "source index вышел за границы".to_owned(),
            })?;
        if let Some(entry) = source.current() {
            self.heap.push(HeapItem {
                entry: entry.clone(),
                source_index,
            });
        }
        Ok(())
    }

    fn consume_item(&mut self, item: HeapItem) -> Result<TableEntry> {
        let source = self
            .sources
            .get_mut(item.source_index)
            .ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-MERGE-SOURCE",
                reason: "source index исчез во время merge".to_owned(),
            })?;
        let current = source.current().ok_or(Error::InternalInvariant {
            invariant: "I-COMPACTION-MERGE-CURRENT",
            reason: "heap ссылается на exhausted source".to_owned(),
        })?;
        if current != &item.entry {
            return Err(Error::InternalInvariant {
                invariant: "I-COMPACTION-MERGE-HEAP-CURRENT",
                reason: "heap entry не совпадает с current source entry".to_owned(),
            });
        }
        let consumed = current.clone();
        source.advance()?;
        if let Some(next) = source.current() {
            if next.internal_key <= consumed.internal_key {
                return Err(Error::corruption(
                    "compaction source ordering",
                    "internal keys source не строго возрастают",
                ));
            }
            self.heap.push(HeapItem {
                entry: next.clone(),
                source_index: item.source_index,
            });
        }
        Ok(consumed)
    }

    fn fill_pending(&mut self) -> Result<bool> {
        let Some(first_item) = self.heap.pop() else {
            return Ok(false);
        };
        let first = self.consume_item(first_item)?;
        let logical_key = LogicalKey::from_entry(&first);
        let mut versions = Vec::new();
        versions.push(first);

        while self
            .heap
            .peek()
            .is_some_and(|item| logical_key.matches(&item.entry))
        {
            let item = self.heap.pop().ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-MERGE-PEEK-POP",
                reason: "heap item исчез после успешного peek".to_owned(),
            })?;
            let entry = self.consume_item(item)?;
            if versions
                .last()
                .is_some_and(|previous| previous.internal_key.sequence() == entry.internal_key.sequence())
            {
                return Err(Error::corruption(
                    "compaction merge",
                    "один logical key содержит duplicate sequence",
                ));
            }
            versions.push(entry);
            if versions.len() > self.options.max_versions_per_user_key {
                return Err(Error::ResourceLimit {
                    resource: "compaction_versions_per_user_key",
                    limit: u64::try_from(self.options.max_versions_per_user_key)
                        .unwrap_or(u64::MAX),
                    observed: u64::try_from(versions.len()).unwrap_or(u64::MAX),
                });
            }
        }

        self.observe_input_group(&versions)?;
        let retained = retain_versions(versions, self.options);
        self.observe_output_group(&retained)?;
        self.pending.extend(retained);
        Ok(true)
    }

    fn observe_input_group(&mut self, versions: &[TableEntry]) -> Result<()> {
        self.stats.user_keys = checked_add(self.stats.user_keys, 1, "compaction_user_keys")?;
        self.stats.input_entries = checked_add(
            self.stats.input_entries,
            u64::try_from(versions.len()).map_err(|_| Error::ResourceLimit {
                resource: "compaction_input_entries",
                limit: u64::MAX,
                observed: u64::MAX,
            })?,
            "compaction_input_entries",
        )?;
        self.stats.peak_versions_per_user_key = self
            .stats
            .peak_versions_per_user_key
            .max(versions.len());
        Ok(())
    }

    fn observe_output_group(&mut self, retained: &[TableEntry]) -> Result<()> {
        let output = u64::try_from(retained.len()).map_err(|_| Error::ResourceLimit {
            resource: "compaction_output_entries",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        self.stats.output_entries = checked_add(
            self.stats.output_entries,
            output,
            "compaction_output_entries",
        )?;
        let dropped = self
            .stats
            .input_entries
            .checked_sub(self.stats.output_entries)
            .ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-DROPPED-COUNT",
                reason: "output entries превышают input entries".to_owned(),
            })?;
        self.stats.dropped_versions = dropped;
        Ok(())
    }
}

impl Iterator for CompactionMerge {
    type Item = Result<TableEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        loop {
            if let Some(entry) = self.pending.pop_front() {
                return Some(Ok(entry));
            }
            match self.fill_pending() {
                Ok(true) => {}
                Ok(false) => return None,
                Err(error) => {
                    self.failed = true;
                    return Some(Err(error));
                }
            }
        }
    }
}

fn retain_versions(
    versions: Vec<TableEntry>,
    options: CompactionMergeOptions,
) -> Vec<TableEntry> {
    let mut retained = Vec::new();
    match options.oldest_snapshot {
        None => {
            if let Some(newest) = versions.into_iter().next()
                && !(options.bottommost
                    && newest.internal_key.value_type() == ValueType::Deletion)
            {
                retained.push(newest);
            }
        }
        Some(oldest_snapshot) => {
            let mut floor_selected = false;
            for entry in versions {
                if entry.internal_key.sequence() > oldest_snapshot {
                    retained.push(entry);
                    continue;
                }
                if floor_selected {
                    continue;
                }
                floor_selected = true;
                if !(options.bottommost
                    && entry.internal_key.value_type() == ValueType::Deletion)
                {
                    retained.push(entry);
                }
            }
        }
    }
    retained
}

fn checked_add(left: u64, right: u64, resource: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or(Error::ResourceLimit {
        resource,
        limit: u64::MAX,
        observed: u64::MAX,
    })
}

#[cfg(test)]
mod tests {
    use forgedb_core::{InternalKey, Limits};

    use super::*;

    fn entry(key: &[u8], sequence: u64, value_type: ValueType, value: &[u8]) -> Result<TableEntry> {
        Ok(TableEntry {
            internal_key: InternalKey::new(
                ColumnFamilyId::DEFAULT,
                key,
                SequenceNumber::new(sequence)?,
                value_type,
                &Limits::default(),
            )?,
            value: value.to_vec(),
        })
    }

    fn source(entries: Vec<TableEntry>) -> Box<dyn CompactionCursor> {
        Box::new(VecCompactionCursor::new(entries))
    }

    fn collect(merge: &mut CompactionMerge) -> Result<Vec<TableEntry>> {
        merge.by_ref().collect()
    }

    #[test]
    fn merge_without_snapshots_keeps_only_newest_version() -> Result<()> {
        let mut merge = CompactionMerge::new(
            vec![
                source(vec![
                    entry(b"a", 5, ValueType::Value, b"new")?,
                    entry(b"a", 3, ValueType::Value, b"old")?,
                ]),
                source(vec![entry(b"b", 4, ValueType::Value, b"b")?]),
            ],
            CompactionMergeOptions::default(),
        )?;
        let output = collect(&mut merge)?;
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].internal_key.user_key(), b"a");
        assert_eq!(output[0].value, b"new");
        assert_eq!(output[1].internal_key.user_key(), b"b");
        assert_eq!(merge.stats().dropped_versions, 1);
        Ok(())
    }

    #[test]
    fn oldest_snapshot_keeps_all_newer_versions_and_one_floor() -> Result<()> {
        let mut merge = CompactionMerge::new(
            vec![source(vec![
                entry(b"key", 9, ValueType::Value, b"v9")?,
                entry(b"key", 7, ValueType::Deletion, b"")?,
                entry(b"key", 5, ValueType::Value, b"v5")?,
                entry(b"key", 3, ValueType::Value, b"v3")?,
            ])],
            CompactionMergeOptions {
                oldest_snapshot: Some(SequenceNumber::new(6)?),
                ..CompactionMergeOptions::default()
            },
        )?;
        let output = collect(&mut merge)?;
        let sequences = output
            .iter()
            .map(|entry| entry.internal_key.sequence().get())
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![9, 7, 5]);
        assert_eq!(merge.stats().dropped_versions, 1);
        Ok(())
    }

    #[test]
    fn bottommost_tombstone_at_snapshot_floor_is_dropped() -> Result<()> {
        let mut merge = CompactionMerge::new(
            vec![source(vec![
                entry(b"key", 9, ValueType::Value, b"v9")?,
                entry(b"key", 5, ValueType::Deletion, b"")?,
                entry(b"key", 3, ValueType::Value, b"v3")?,
            ])],
            CompactionMergeOptions {
                oldest_snapshot: Some(SequenceNumber::new(6)?),
                bottommost: true,
                ..CompactionMergeOptions::default()
            },
        )?;
        let output = collect(&mut merge)?;
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].internal_key.sequence().get(), 9);
        Ok(())
    }

    #[test]
    fn non_bottommost_tombstone_is_retained() -> Result<()> {
        let mut merge = CompactionMerge::new(
            vec![source(vec![
                entry(b"key", 5, ValueType::Deletion, b"")?,
                entry(b"key", 3, ValueType::Value, b"v3")?,
            ])],
            CompactionMergeOptions::default(),
        )?;
        let output = collect(&mut merge)?;
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].internal_key.value_type(), ValueType::Deletion);
        Ok(())
    }

    #[test]
    fn duplicate_sequence_across_sources_is_corruption() -> Result<()> {
        let mut merge = CompactionMerge::new(
            vec![
                source(vec![entry(b"key", 5, ValueType::Value, b"left")?]),
                source(vec![entry(b"key", 5, ValueType::Value, b"right")?]),
            ],
            CompactionMergeOptions::default(),
        )?;
        assert!(matches!(merge.next(), Some(Err(Error::Corruption { .. }))));
        assert!(merge.next().is_none());
        Ok(())
    }

    #[test]
    fn versions_per_key_limit_is_enforced() -> Result<()> {
        let mut merge = CompactionMerge::new(
            vec![source(vec![
                entry(b"key", 3, ValueType::Value, b"3")?,
                entry(b"key", 2, ValueType::Value, b"2")?,
                entry(b"key", 1, ValueType::Value, b"1")?,
            ])],
            CompactionMergeOptions {
                max_versions_per_user_key: 2,
                ..CompactionMergeOptions::default()
            },
        )?;
        assert!(matches!(
            merge.next(),
            Some(Err(Error::ResourceLimit { .. }))
        ));
        Ok(())
    }

    #[test]
    fn invalid_source_order_is_rejected_incrementally() -> Result<()> {
        let mut merge = CompactionMerge::new(
            vec![source(vec![
                entry(b"b", 1, ValueType::Value, b"b")?,
                entry(b"a", 1, ValueType::Value, b"a")?,
            ])],
            CompactionMergeOptions::default(),
        )?;
        assert!(matches!(merge.next(), Some(Err(Error::Corruption { .. }))));
        Ok(())
    }

    #[test]
    fn source_limit_is_checked_before_heap_initialization() {
        let result = CompactionMerge::new(
            vec![source(Vec::new()), source(Vec::new())],
            CompactionMergeOptions {
                max_sources: 1,
                ..CompactionMergeOptions::default()
            },
        );
        assert!(matches!(result, Err(Error::ResourceLimit { .. })));
    }
}
