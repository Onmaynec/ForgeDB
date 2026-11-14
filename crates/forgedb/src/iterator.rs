use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::ops::{Bound, RangeBounds};
use std::sync::Arc;

use forgedb_core::{Error, InternalKey, Result, SequenceNumber, ValueType};
use forgedb_sstable::{TableEntry, TableMeta};

use crate::entry_cursor::{EntryCursor, VecEntryCursor};
use crate::version_set::VersionSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RangePosition {
    Before,
    Inside,
    After,
}

/// Owned границы одного forward range scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OwnedRange {
    start: Bound<Vec<u8>>,
    end: Bound<Vec<u8>>,
}

impl OwnedRange {
    pub(crate) fn from_range<R>(range: &R) -> Result<Self>
    where
        R: RangeBounds<Vec<u8>>,
    {
        let start = match range.start_bound() {
            Bound::Included(value) => Bound::Included(value.clone()),
            Bound::Excluded(value) => Bound::Excluded(value.clone()),
            Bound::Unbounded => Bound::Unbounded,
        };
        let end = match range.end_bound() {
            Bound::Included(value) => Bound::Included(value.clone()),
            Bound::Excluded(value) => Bound::Excluded(value.clone()),
            Bound::Unbounded => Bound::Unbounded,
        };
        let owned = Self { start, end };
        owned.validate()?;
        Ok(owned)
    }

    pub(crate) fn all() -> Self {
        Self {
            start: Bound::Unbounded,
            end: Bound::Unbounded,
        }
    }

    fn position(&self, key: &[u8]) -> RangePosition {
        let before_start = match &self.start {
            Bound::Included(start) => key < start.as_slice(),
            Bound::Excluded(start) => key <= start.as_slice(),
            Bound::Unbounded => false,
        };
        if before_start {
            return RangePosition::Before;
        }
        let after_end = match &self.end {
            Bound::Included(end) => key > end.as_slice(),
            Bound::Excluded(end) => key >= end.as_slice(),
            Bound::Unbounded => false,
        };
        if after_end {
            RangePosition::After
        } else {
            RangePosition::Inside
        }
    }

    /// Консервативные границы для binary SST range planning.
    /// Excluded bounds обрабатываются точным overlaps() после pruning;
    /// здесь можно взять лишнюю пограничную таблицу, но не пропустить нужную.
    pub(crate) fn pruning_bounds(&self) -> (Option<&[u8]>, Option<&[u8]>) {
        let start = match &self.start {
            Bound::Included(key) | Bound::Excluded(key) => Some(key.as_slice()),
            Bound::Unbounded => None,
        };
        let end = match &self.end {
            Bound::Included(key) | Bound::Excluded(key) => Some(key.as_slice()),
            Bound::Unbounded => None,
        };
        (start, end)
    }

    pub(crate) fn overlaps(&self, metadata: &TableMeta) -> bool {
        let ends_before_start = match &self.start {
            Bound::Included(start) => metadata.largest_user_key.as_slice() < start.as_slice(),
            Bound::Excluded(start) => metadata.largest_user_key.as_slice() <= start.as_slice(),
            Bound::Unbounded => false,
        };
        let starts_after_end = match &self.end {
            Bound::Included(end) => metadata.smallest_user_key.as_slice() > end.as_slice(),
            Bound::Excluded(end) => metadata.smallest_user_key.as_slice() >= end.as_slice(),
            Bound::Unbounded => false,
        };
        !ends_before_start && !starts_after_end
    }

    fn validate(&self) -> Result<()> {
        let start = match &self.start {
            Bound::Included(value) | Bound::Excluded(value) => Some(value),
            Bound::Unbounded => None,
        };
        let end = match &self.end {
            Bound::Included(value) | Bound::Excluded(value) => Some(value),
            Bound::Unbounded => None,
        };
        if start.zip(end).is_some_and(|(start, end)| start > end) {
            return Err(Error::invalid_argument(
                "range",
                "start bound больше end bound",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct SourceCursor {
    cursor: Box<dyn EntryCursor>,
    priority: usize,
    previous: Option<InternalKey>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HeapItem {
    key: InternalKey,
    source_index: usize,
    priority: usize,
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .key
            .cmp(&self.key)
            .then_with(|| other.priority.cmp(&self.priority))
            .then_with(|| other.source_index.cmp(&self.source_index))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Forward user-key iterator, объединяющий sorted MVCC cursors через binary heap.
#[derive(Debug)]
pub struct DbIterator {
    sources: Vec<SourceCursor>,
    heap: BinaryHeap<HeapItem>,
    snapshot: Option<SequenceNumber>,
    range: OwnedRange,
    limit: Option<usize>,
    emitted: usize,
    failed: bool,
    // Пинним поколение файлов для Windows-safe lifetime SST cursors.
    _version_lease: Option<Arc<VersionSet>>,
}

impl DbIterator {
    pub(crate) fn new(
        sources: Vec<(usize, Vec<TableEntry>)>,
        snapshot: Option<SequenceNumber>,
        range: OwnedRange,
        limit: Option<usize>,
    ) -> Result<Self> {
        let cursors = sources
            .into_iter()
            .map(|(priority, entries)| {
                (
                    priority,
                    Box::new(VecEntryCursor::new(entries)) as Box<dyn EntryCursor>,
                )
            })
            .collect();
        Self::new_cursors(cursors, snapshot, range, limit)
    }

    pub(crate) fn new_cursors(
        sources: Vec<(usize, Box<dyn EntryCursor>)>,
        snapshot: Option<SequenceNumber>,
        range: OwnedRange,
        limit: Option<usize>,
    ) -> Result<Self> {
        if limit == Some(0) {
            return Ok(Self::empty(snapshot, limit));
        }
        let mut iterator = Self {
            sources: sources
                .into_iter()
                .map(|(priority, cursor)| SourceCursor {
                    cursor,
                    priority,
                    previous: None,
                })
                .collect(),
            heap: BinaryHeap::new(),
            snapshot,
            range,
            limit,
            emitted: 0,
            failed: false,
            _version_lease: None,
        };
        for source_index in 0..iterator.sources.len() {
            iterator.queue_current(source_index)?;
        }
        Ok(iterator)
    }

    pub(crate) fn empty(snapshot: Option<SequenceNumber>, limit: Option<usize>) -> Self {
        Self {
            sources: Vec::new(),
            heap: BinaryHeap::new(),
            snapshot,
            range: OwnedRange::all(),
            limit,
            emitted: 0,
            failed: false,
            _version_lease: None,
        }
    }

    pub(crate) fn with_version_lease(mut self, version: Arc<VersionSet>) -> Self {
        self._version_lease = Some(version);
        self
    }

    fn queue_current(&mut self, source_index: usize) -> Result<()> {
        loop {
            let (position, key, priority) = {
                let source = self
                    .sources
                    .get(source_index)
                    .ok_or(Error::InternalInvariant {
                        invariant: "I-MERGE-SOURCE-INDEX",
                        reason: "source index выходит за границы".to_owned(),
                    })?;
                let Some(entry) = source.cursor.current() else {
                    return Ok(());
                };
                (
                    self.range.position(entry.internal_key.user_key()),
                    entry.internal_key.clone(),
                    source.priority,
                )
            };
            match position {
                RangePosition::Before => self.advance_source(source_index)?,
                RangePosition::Inside => {
                    self.heap.push(HeapItem {
                        key,
                        source_index,
                        priority,
                    });
                    return Ok(());
                }
                RangePosition::After => return Ok(()),
            }
        }
    }

    fn advance_source(&mut self, source_index: usize) -> Result<()> {
        let source = self
            .sources
            .get_mut(source_index)
            .ok_or(Error::InternalInvariant {
                invariant: "I-MERGE-SOURCE-INDEX",
                reason: "source index выходит за границы".to_owned(),
            })?;
        // Проверка порядка требует только internal key. Раньше здесь
        // клонировали TableEntry целиком, включая потенциально огромный value.
        let current_key = source
            .cursor
            .current()
            .map(|entry| entry.internal_key.clone())
            .ok_or(Error::InternalInvariant {
                invariant: "I-MERGE-SOURCE-POSITION",
                reason: "advance вызван для завершённого source".to_owned(),
            })?;
        if source
            .previous
            .as_ref()
            .is_some_and(|previous| previous >= &current_key)
        {
            return Err(Error::corruption(
                "merge iterator source",
                "source internal keys не строго возрастают",
            ));
        }
        source.previous = Some(current_key);
        source.cursor.advance()
    }

    fn consume_item(&mut self, item: HeapItem, retain_value: bool) -> Result<Option<TableEntry>> {
        // В невидимых MVCC версиях и уже скрытых записях не копируем value:
        // валидируем heap key через ссылку и сразу двигаем cursor дальше.
        let entry = self
            .sources
            .get(item.source_index)
            .and_then(|source| source.cursor.current())
            .ok_or(Error::InternalInvariant {
                invariant: "I-MERGE-SOURCE-POSITION",
                reason: "heap ссылается на завершённый source".to_owned(),
            })?;
        if entry.internal_key != item.key {
            return Err(Error::InternalInvariant {
                invariant: "I-MERGE-HEAP-KEY",
                reason: "heap key не совпадает с source cursor".to_owned(),
            });
        }
        let visible_entry = retain_value.then(|| entry.clone());
        self.advance_source(item.source_index)?;
        self.queue_current(item.source_index)?;
        Ok(visible_entry)
    }
}

impl Iterator for DbIterator {
    type Item = Result<(Vec<u8>, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.limit.is_some_and(|limit| self.emitted >= limit) {
            return None;
        }
        let snapshot = self.snapshot?;

        loop {
            let first = self.heap.pop()?;
            let user_key = first.key.user_key().to_vec();
            let mut visible: Option<TableEntry> = None;
            let first_visible = first.key.sequence() <= snapshot;
            match self.consume_item(first, first_visible) {
                Ok(Some(entry)) => visible = Some(entry),
                Ok(None) => {}
                Err(error) => {
                    self.failed = true;
                    return Some(Err(error));
                }
            }

            while self
                .heap
                .peek()
                .is_some_and(|item| item.key.user_key() == user_key.as_slice())
            {
                let Some(item) = self.heap.pop() else {
                    self.failed = true;
                    return Some(Err(Error::InternalInvariant {
                        invariant: "I-MERGE-HEAP-PEEK-POP",
                        reason: "heap изменился между peek и pop".to_owned(),
                    }));
                };
                let retain_value = visible.is_none() && item.key.sequence() <= snapshot;
                match self.consume_item(item, retain_value) {
                    Ok(Some(entry)) => visible = Some(entry),
                    Ok(None) => {}
                    Err(error) => {
                        self.failed = true;
                        return Some(Err(error));
                    }
                }
            }

            let Some(entry) = visible else {
                continue;
            };
            match entry.internal_key.value_type() {
                ValueType::Deletion => continue,
                ValueType::Value => {
                    self.emitted = match self.emitted.checked_add(1) {
                        Some(value) => value,
                        None => {
                            self.failed = true;
                            return Some(Err(Error::ResourceLimit {
                                resource: "scan_result_count",
                                limit: u64::MAX,
                                observed: u64::MAX,
                            }));
                        }
                    };
                    return Some(Ok((user_key, entry.value)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use forgedb_core::{Limits, SequenceNumber};

    use super::*;

    fn value(key: &[u8], sequence: u64, value: &[u8]) -> Result<TableEntry> {
        TableEntry::value(
            key.to_vec(),
            SequenceNumber::new(sequence)?,
            value.to_vec(),
            &Limits::default(),
        )
    }

    fn tombstone(key: &[u8], sequence: u64) -> Result<TableEntry> {
        TableEntry::tombstone(
            key.to_vec(),
            SequenceNumber::new(sequence)?,
            &Limits::default(),
        )
    }

    #[test]
    fn merge_hides_old_versions_and_tombstones() -> Result<()> {
        let newest = vec![value(b"a", 5, b"new")?, tombstone(b"b", 4)?];
        let older = vec![
            value(b"a", 2, b"old")?,
            value(b"b", 1, b"visible-before-delete")?,
            value(b"c", 3, b"three")?,
        ];
        let iterator = DbIterator::new(
            vec![(0, newest), (1, older)],
            Some(SequenceNumber::new(5)?),
            OwnedRange::all(),
            None,
        )?;
        let rows = iterator.collect::<Result<Vec<_>>>()?;
        assert_eq!(
            rows,
            vec![
                (b"a".to_vec(), b"new".to_vec()),
                (b"c".to_vec(), b"three".to_vec())
            ]
        );
        Ok(())
    }

    #[test]
    fn snapshot_selects_older_version() -> Result<()> {
        let source = vec![value(b"key", 9, b"new")?, value(b"key", 3, b"old")?];
        let iterator = DbIterator::new(
            vec![(0, source)],
            Some(SequenceNumber::new(3)?),
            OwnedRange::all(),
            None,
        )?;
        let rows = iterator.collect::<Result<Vec<_>>>()?;
        assert_eq!(rows, vec![(b"key".to_vec(), b"old".to_vec())]);
        Ok(())
    }

    #[test]
    fn range_filtering_is_incremental() -> Result<()> {
        let source = vec![
            value(b"a", 1, b"one")?,
            value(b"b", 2, b"two")?,
            value(b"c", 3, b"three")?,
        ];
        let range = OwnedRange::from_range(&(b"b".to_vec()..b"c".to_vec()))?;
        let iterator = DbIterator::new(
            vec![(0, source)],
            Some(SequenceNumber::new(3)?),
            range,
            None,
        )?;
        let rows = iterator.collect::<Result<Vec<_>>>()?;
        assert_eq!(rows, vec![(b"b".to_vec(), b"two".to_vec())]);
        Ok(())
    }

    #[test]
    fn invalid_source_order_is_reported_during_iteration() -> Result<()> {
        let source = vec![value(b"b", 2, b"two")?, value(b"a", 1, b"one")?];
        let iterator = DbIterator::new(
            vec![(0, source)],
            Some(SequenceNumber::new(2)?),
            OwnedRange::all(),
            None,
        )?;
        assert!(iterator.collect::<Result<Vec<_>>>().is_err());
        Ok(())
    }
}
