use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use forgedb_core::{DatabaseId, Error, Result, SequenceNumber};

/// Дешёвый immutable handle MVCC snapshot.
#[derive(Clone, Debug)]
pub struct Snapshot {
    identity: DatabaseId,
    handle_id: DatabaseId,
    sequence: Option<SequenceNumber>,
    _lease: Arc<SnapshotLease>,
}

impl Snapshot {
    /// Возвращает sequence snapshot. `None` означает snapshot пустой базы до первой записи.
    #[must_use]
    pub const fn sequence(&self) -> Option<SequenceNumber> {
        self.sequence
    }

    /// Возвращает persisted identity базы, создавшей snapshot.
    #[must_use]
    pub const fn database_id(&self) -> DatabaseId {
        self.identity
    }

    pub(crate) fn belongs_to(&self, identity: DatabaseId, handle_id: DatabaseId) -> bool {
        self.identity == identity && self.handle_id == handle_id
    }
}

#[derive(Debug)]
struct SnapshotLease {
    registry: Weak<SnapshotRegistry>,
    key: u64,
}

impl Drop for SnapshotLease {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let Ok(mut active) = registry.active.lock() else {
            return;
        };
        let should_remove = match active.get_mut(&self.key) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => true,
            None => false,
        };
        if should_remove {
            active.remove(&self.key);
        }
    }
}

/// Registry active snapshots. Ключ `0` зарезервирован для snapshot пустой базы.
#[derive(Debug, Default)]
pub(crate) struct SnapshotRegistry {
    active: Mutex<BTreeMap<u64, usize>>,
}

impl SnapshotRegistry {
    pub(crate) fn register(
        self: &Arc<Self>,
        identity: DatabaseId,
        handle_id: DatabaseId,
        sequence: Option<SequenceNumber>,
    ) -> Result<Snapshot> {
        let key = sequence.map_or(0, SequenceNumber::get);
        let mut active = self.active.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-SNAPSHOT-REGISTRY-MUTEX",
            reason: "mutex registry snapshots poisoned".to_owned(),
        })?;
        let count = active.entry(key).or_insert(0);
        *count = count.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "active_snapshots",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        drop(active);
        Ok(Snapshot {
            identity,
            handle_id,
            sequence,
            _lease: Arc::new(SnapshotLease {
                registry: Arc::downgrade(self),
                key,
            }),
        })
    }

    pub(crate) fn oldest_active_sequence(&self) -> Result<Option<SequenceNumber>> {
        let active = self.active.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-SNAPSHOT-REGISTRY-MUTEX",
            reason: "mutex registry snapshots poisoned".to_owned(),
        })?;
        match active.keys().next().copied() {
            None | Some(0) => Ok(None),
            Some(value) => SequenceNumber::new(value).map(Some),
        }
    }

    #[cfg(test)]
    pub(crate) fn active_handle_count(&self) -> Result<usize> {
        let active = self.active.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-SNAPSHOT-REGISTRY-MUTEX",
            reason: "mutex registry snapshots poisoned".to_owned(),
        })?;
        active.values().try_fold(0_usize, |total, count| {
            total.checked_add(*count).ok_or(Error::ResourceLimit {
                resource: "active_snapshots",
                limit: u64::MAX,
                observed: u64::MAX,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_snapshot_uses_one_registry_lease() -> Result<()> {
        let registry = Arc::new(SnapshotRegistry::default());
        let identity = DatabaseId::random();
        let handle_id = DatabaseId::random();
        let snapshot = registry.register(identity, handle_id, Some(SequenceNumber::FIRST))?;
        let clone = snapshot.clone();
        assert_eq!(registry.active_handle_count()?, 1);
        drop(snapshot);
        assert_eq!(registry.active_handle_count()?, 1);
        drop(clone);
        assert_eq!(registry.active_handle_count()?, 0);
        Ok(())
    }

    #[test]
    fn oldest_snapshot_is_reported() -> Result<()> {
        let registry = Arc::new(SnapshotRegistry::default());
        let identity = DatabaseId::random();
        let handle_id = DatabaseId::random();
        let newer = registry.register(identity, handle_id, Some(SequenceNumber::new(9)?))?;
        let older = registry.register(identity, handle_id, Some(SequenceNumber::new(3)?))?;
        assert_eq!(registry.oldest_active_sequence()?, Some(SequenceNumber::new(3)?));
        drop(older);
        assert_eq!(registry.oldest_active_sequence()?, Some(SequenceNumber::new(9)?));
        drop(newer);
        Ok(())
    }
}
