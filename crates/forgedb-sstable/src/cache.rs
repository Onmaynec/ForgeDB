//! Ограниченный LRU-кэш декодированных SSTable data blocks.
//!
//! Ключ — неизменяемый номер SSTable и позиция блока в его проверенном индексе.
//! Кэш относится к одному открытому экземпляру базы: между базами не разделяется.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::sync::{Arc, Mutex};

use forgedb_core::{Error, Result};

use crate::TableEntry;

type BlockKey = (u64, usize);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub bypasses: u64,
    pub resident_blocks: usize,
    pub resident_bytes: usize,
    pub capacity_bytes: usize,
}

#[derive(Debug)]
struct CachedBlock {
    entries: Arc<Vec<TableEntry>>,
    charge: usize,
    access: u64,
}

#[derive(Debug, Default)]
struct CacheState {
    blocks: BTreeMap<BlockKey, CachedBlock>,
    lru: BTreeSet<(u64, BlockKey)>,
    bytes: usize,
    clock: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    bypasses: u64,
}

impl CacheState {
    fn next_access(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }
}

/// LRU-кэш, учитывающий размер декодированных blocks.
///
/// В лимит входят контейнер записей и ёмкости ключей/значений; временные
/// результаты выполняющихся reads живут отдельно от residency кэша.
#[derive(Debug)]
pub struct BlockCache {
    capacity_bytes: usize,
    state: Mutex<CacheState>,
}

impl BlockCache {
    #[must_use]
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            capacity_bytes,
            state: Mutex::new(CacheState::default()),
        }
    }

    pub fn stats(&self) -> Result<CacheStats> {
        let state = self.lock_state()?;
        Ok(CacheStats {
            hits: state.hits,
            misses: state.misses,
            evictions: state.evictions,
            bypasses: state.bypasses,
            resident_blocks: state.blocks.len(),
            resident_bytes: state.bytes,
            capacity_bytes: self.capacity_bytes,
        })
    }

    pub(crate) fn get_or_decode<F>(
        &self,
        file_number: u64,
        block_index: usize,
        decode: F,
    ) -> Result<Arc<Vec<TableEntry>>>
    where
        F: FnOnce() -> Result<Vec<TableEntry>>,
    {
        let key = (file_number, block_index);
        {
            let mut state = self.lock_state()?;
            if let Some(block) = state.blocks.get(&key) {
                let entries = Arc::clone(&block.entries);
                let previous = block.access;
                state.lru.remove(&(previous, key));
                let current = state.next_access();
                if let Some(block) = state.blocks.get_mut(&key) {
                    block.access = current;
                }
                state.lru.insert((current, key));
                state.hits = state.hits.saturating_add(1);
                return Ok(entries);
            }
            state.misses = state.misses.saturating_add(1);
        }

        // Декодирование и checksum validation не удерживают cache mutex.
        // Неудачный decode никогда не помещается в кэш.
        let decoded = Arc::new(decode()?);
        let charge = estimated_bytes(decoded.as_ref())?;
        let mut state = self.lock_state()?;
        if self.capacity_bytes == 0 || charge > self.capacity_bytes {
            state.bypasses = state.bypasses.saturating_add(1);
            return Ok(decoded);
        }
        // Второй reader мог установить тот же immutable block, пока шёл decode.
        if let Some(existing) = state.blocks.get(&key) {
            return Ok(Arc::clone(&existing.entries));
        }
        while state.bytes > self.capacity_bytes.saturating_sub(charge) {
            let Some((access, oldest)) = state.lru.iter().next().copied() else {
                return Err(Error::InternalInvariant {
                    invariant: "I-CACHE-LRU-NONEMPTY",
                    reason: "занятые bytes без записей в LRU".to_owned(),
                });
            };
            state.lru.remove(&(access, oldest));
            if let Some(discarded) = state.blocks.remove(&oldest) {
                state.bytes = state.bytes.saturating_sub(discarded.charge);
                state.evictions = state.evictions.saturating_add(1);
            }
        }
        let access = state.next_access();
        state.bytes = state
            .bytes
            .checked_add(charge)
            .ok_or(Error::ResourceLimit {
                resource: "block_cache_bytes",
                limit: u64::try_from(self.capacity_bytes).unwrap_or(u64::MAX),
                observed: u64::MAX,
            })?;
        state.blocks.insert(
            key,
            CachedBlock {
                entries: Arc::clone(&decoded),
                charge,
                access,
            },
        );
        state.lru.insert((access, key));
        Ok(decoded)
    }

    /// Выбрасывает кэшированные blocks физически удалённого SSTable.
    /// File numbers никогда не переиспользуются, но это освобождает RAM после cleanup.
    pub fn evict_table(&self, file_number: u64) -> Result<()> {
        let mut state = self.lock_state()?;
        let keys = state
            .blocks
            .keys()
            .filter(|(number, _)| *number == file_number)
            .copied()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(discarded) = state.blocks.remove(&key) {
                state.lru.remove(&(discarded.access, key));
                state.bytes = state.bytes.saturating_sub(discarded.charge);
                state.evictions = state.evictions.saturating_add(1);
            }
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, CacheState>> {
        self.state.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-BLOCK-CACHE-LOCK",
            reason: "mutex block cache poisoned".to_owned(),
        })
    }
}

fn estimated_bytes(entries: &Vec<TableEntry>) -> Result<usize> {
    let mut sum = mem::size_of::<Vec<TableEntry>>()
        .checked_add(
            entries
                .capacity()
                .saturating_mul(mem::size_of::<TableEntry>()),
        )
        .ok_or(Error::ResourceLimit {
            resource: "block_cache_bytes",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    for entry in entries {
        sum = sum
            .checked_add(entry.internal_key.user_key().len())
            .and_then(|size| size.checked_add(entry.value.capacity()))
            .ok_or(Error::ResourceLimit {
                resource: "block_cache_bytes",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use forgedb_core::{Limits, SequenceNumber};

    use super::*;

    fn block(name: &'static [u8]) -> Result<Vec<TableEntry>> {
        Ok(vec![TableEntry::value(
            name.to_vec(),
            SequenceNumber::FIRST,
            vec![b'x'; 80],
            &Limits::default(),
        )?])
    }

    #[test]
    fn warm_block_hits_and_bounded_eviction() -> Result<()> {
        let cache = BlockCache::new(310);
        let first = cache.get_or_decode(1, 0, || block(b"a"))?;
        assert_eq!(first.len(), 1);
        let second = cache.get_or_decode(1, 0, || {
            Err(Error::corruption(
                "test",
                "попытка повторно декодировать cached block",
            ))
        })?;
        assert_eq!(second.len(), 1);
        for index in 1..8 {
            cache.get_or_decode(1, index, || block(b"b"))?;
        }
        let stats = cache.stats()?;
        assert_eq!(stats.hits, 1);
        assert!(stats.evictions > 0);
        assert!(stats.resident_bytes <= stats.capacity_bytes);
        Ok(())
    }

    #[test]
    fn disabled_and_oversized_cache_never_retain_blocks() -> Result<()> {
        let cache = BlockCache::new(0);
        cache.get_or_decode(2, 0, || block(b"c"))?;
        cache.get_or_decode(2, 0, || block(b"c"))?;
        let stats = cache.stats()?;
        assert_eq!(stats.resident_blocks, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.bypasses, 2);
        Ok(())
    }

    #[test]
    fn decode_failure_is_not_cached() -> Result<()> {
        let cache = BlockCache::new(4096);
        assert!(
            cache
                .get_or_decode(3, 0, || {
                    Err(Error::corruption("test", "corrupted data block"))
                })
                .is_err()
        );
        cache.get_or_decode(3, 0, || block(b"d"))?;
        let stats = cache.stats()?;
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.resident_blocks, 1);
        Ok(())
    }
}
