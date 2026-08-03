use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::ops::{Bound, RangeBounds};

use forgedb_core::{Error, InternalKey, Result, SequenceNumber, ValueType};
use forgedb_sstable::{TableEntry, TableMeta};

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

    pub(crate) fn contains(&self, key: &[u8]) -> bool {
        let after_start = match &self.start {
            Bound::Included(start) => key >= start.as_slice(),
            Bound::Excluded(start) => key > start.as_slice(),
            Bound::Unbounded => true,
        };
        let before_end = match &self.end {
            Bound::Included(end) => key <= end.as_slice(),
            Bound::Excluded(end) => key < end.as_slice(),
            Bound::Unbounded => true,
        };
        after_start && before_end
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

#[derive(Clone, Debug)]
struct SourceCursor {
    entries: Vec<TableEntry>,
    position: usize,
    priority: usize,
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

/// Forward user-key iterator, объединяющий sorted MVCC sources через binary heap.
#[derive(Debug)]
pub struct DbIterator {
    sources: Vec<SourceCursor>,
    heap: BinaryHeap<HeapItem>,
    snapshot: Option<SequenceNumber>,
    limit: Option<usize>,
    emitted: usize,
    failed: bool,
}

impl DbIterator {
    pub(crate) fn new(
        sources: Vec<(usize, Vec<TableEntry>)>,
        snapshot: Option<SequenceNumber>,
        range: OwnedRange,
        limit: Option<usize>,
    ) -> Result<Self> {
        if limit == Some(0) {
            return Ok(Self::empty(snapshot, limit));
        }
        let mut cursors = Vec::with_capacity(sources.len());
        for (priority, mut entries) in sources {
            entries.retain(|entry| range.contains(entry.internal_key.user_key()));
            if entries
                .windows(2)
                .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(left, right)| {
                    left.internal_key >= right.internal_key
                }))
            {
                return Err(Error::corruption(
                    "merge iterator source",
                    "source internal keys не строго возрастают",
                ));
            }
            cursors.push(SourceCursor {
                entries,
                position: 0,
                priority,
            });
        }
        let mut iterator = Self {
            sources: cursors,
            heap: BinaryHeap::new(),
            snapshot,
            limit,
            emitted: 0,
            failed: false,
        };
        for source_index in 0..iterator.sources.len() {
            iterator.push_current(source_index)?;
        }
        Ok(iterator)
    }

    pub(crate) fn empty(snapshot: Option<SequenceNumber>, limit: Option<usize>) -> Self {
        Self {
            sources: Vec::new(),
            heap: BinaryHeap::new(),
            snapshot,
            limit,
            emitted: 0,
            failed: false,
        }
    }

    fn push_current(&mut self, source_index: usize) -> Result<()> {
        let source = self
            .sources
            .get(source_index)
            .ok_or(Error::InternalInvariant {
                invariant: "I-MERGE-SOURCE-INDEX",
                reason: "source index выходит за границы".to_owned(),
            })?;
        let Some(entry) = source.entries.get(source.position) else {
            return Ok(());
        };
        self.heap.push(HeapItem {
            key: entry.internal_key.clone(),
            source_index,
            priority: source.priority,
        });
        Ok(())
    }

    fn consume_item(&mut self, item: HeapItem) -> Result<TableEntry> {
        let source = self
            .sources
            .get_mut(item.source_index)
            .ok_or(Error::InternalInvariant {
                invariant: "I-MERGE-SOURCE-INDEX",
                reason: "heap source index выходит за границы".to_owned(),
            })?;
        let entry = source
            .entries
            .get(source.position)
            .cloned()
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
        source.position = source.position.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "merge_source_position",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        self.push_current(item.source_index)?;
        Ok(entry)
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
            match self.consume_item(first) {
                Ok(entry) if entry.internal_key.sequence() <= snapshot => visible = Some(entry),
                Ok(_) => {}
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
                match self.consume_item(item) {
                    Ok(entry)
                        if visible.is_none() && entry.internal_key.sequence() <= snapshot =>
                    {
                        visible = Some(entry);
                    }
                    Ok(_) => {}
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
}
