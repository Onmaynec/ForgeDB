use std::fmt::Debug;

use forgedb_core::{Error, Result};
use forgedb_sstable::TableEntry;

/// Owned sorted source internal entries для merge iterator.
pub(crate) trait EntryCursor: Debug + Send {
    /// Возвращает текущую entry без перемещения cursor.
    fn current(&self) -> Option<&TableEntry>;

    /// Перемещает cursor на следующую entry.
    fn advance(&mut self) -> Result<()>;

    /// Число entries, физически удерживаемых cursor в текущий момент.
    #[cfg(test)]
    fn resident_entries(&self) -> usize;
}

/// Cursor над уже материализованными entries mutable/immutable memtable.
#[derive(Debug)]
pub(crate) struct VecEntryCursor {
    entries: Vec<TableEntry>,
    position: usize,
}

impl VecEntryCursor {
    #[must_use]
    pub(crate) fn new(entries: Vec<TableEntry>) -> Self {
        Self {
            entries,
            position: 0,
        }
    }
}

impl EntryCursor for VecEntryCursor {
    fn current(&self) -> Option<&TableEntry> {
        self.entries.get(self.position)
    }

    fn advance(&mut self) -> Result<()> {
        if self.current().is_none() {
            return Err(Error::InternalInvariant {
                invariant: "I-ENTRY-CURSOR-ADVANCE",
                reason: "попытка advance завершённого VecEntryCursor".to_owned(),
            });
        }
        self.position = self.position.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "entry_cursor_position",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        Ok(())
    }

    #[cfg(test)]
    fn resident_entries(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use forgedb_core::{Limits, SequenceNumber};

    use super::*;

    #[test]
    fn vec_cursor_advances_without_copying_source() -> Result<()> {
        let entries = vec![
            TableEntry::value(
                b"a".to_vec(),
                SequenceNumber::FIRST,
                b"one".to_vec(),
                &Limits::default(),
            )?,
            TableEntry::value(
                b"b".to_vec(),
                SequenceNumber::new(2)?,
                b"two".to_vec(),
                &Limits::default(),
            )?,
        ];
        let mut cursor = VecEntryCursor::new(entries);
        assert_eq!(
            cursor
                .current()
                .map(|entry| entry.internal_key.user_key()),
            Some(b"a".as_slice())
        );
        cursor.advance()?;
        assert_eq!(
            cursor
                .current()
                .map(|entry| entry.internal_key.user_key()),
            Some(b"b".as_slice())
        );
        cursor.advance()?;
        assert!(cursor.current().is_none());
        assert_eq!(cursor.resident_entries(), 2);
        Ok(())
    }
}
