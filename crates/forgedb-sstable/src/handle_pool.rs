//! Bounded native SST read-handle cache (point lookup, не streaming cursors).
//!
//! Один mutex удерживается на время seek/read: это осознанный обмен
//! параллелизма на строгий лимит открытых handles без oversubscription.
//! Старые VersionSet разделяют один pool через Arc, а ownership удаления SST
//! по-прежнему определяется reference-aware TableReader lifetime.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use forgedb_core::{DatabaseId, Error, Result};
use forgedb_fs::{FileSystem, NativeFileSystem};

use crate::{SST_HEADER_SIZE, TableMeta, decode_header, read_exact_at};

/// Диагностика только point-read SST handle pool текущей базы.
/// WAL/MANIFEST, compaction и streaming cursors сюда не входят.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TableHandleStats {
    /// Конфигурационный максимум одновременно хранимых point-read handles.
    pub capacity: usize,
    /// Текущее число open handles внутри LRU.
    pub open_handles: usize,
    /// Повторные чтения уже открытого файла.
    pub hits: u64,
    /// Открытия нового handle, включая повторные после eviction.
    pub misses: u64,
    /// Закрытия LRU handle при достижении capacity.
    pub evictions: u64,
    /// Повторные открытия ранее использованного SST file number.
    pub reopens: u64,
}

#[derive(Debug)]
struct CachedHandle {
    path: std::path::PathBuf,
    file: File,
    access: u64,
}

#[derive(Debug, Default)]
struct PoolState {
    opened: BTreeMap<u64, CachedHandle>,
    seen: BTreeSet<u64>,
    clock: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    reopens: u64,
}

impl PoolState {
    fn tick(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }
}

/// Кэш native point-read файлов, использующий общую mutex-границу.
/// Не кэширует декодированные blocks; этим занимается отдельный BlockCache.
#[derive(Debug)]
pub struct TableHandlePool {
    capacity: usize,
    state: Mutex<PoolState>,
}

impl TableHandlePool {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(PoolState::default()),
        }
    }

    pub fn stats(&self) -> Result<TableHandleStats> {
        let state = self.lock()?;
        Ok(TableHandleStats {
            capacity: self.capacity,
            open_handles: state.opened.len(),
            hits: state.hits,
            misses: state.misses,
            evictions: state.evictions,
            reopens: state.reopens,
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, PoolState>> {
        self.state.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-SST-HANDLE-POOL-MUTEX",
            reason: "SST native handle pool mutex poisoned".to_owned(),
        })
    }

    /// Использовать открытый native File или создать заново после LRU eviction.
    /// Reopen перепроверяет file length, header checksum, DB identity, number,
    /// sequence range. Data block checksum проверяется вызывающим кодом.
    ///
    /// Serialized read гарантирует, что во время callback pooled File не
    /// выселится/не будет удалён другим потоком.
    pub(crate) fn with_file<T>(
        &self,
        path: &Path,
        identity: DatabaseId,
        metadata: &TableMeta,
        read: impl FnOnce(&mut File) -> Result<T>,
    ) -> Result<T> {
        let mut state = self.lock()?;
        let number = metadata.file_number;
        if let Some(existing) = state.opened.get(&number) {
            if existing.path != path {
                return Err(Error::InternalInvariant {
                    invariant: "I-SST-HANDLE-PATH-IDENTITY",
                    reason: "один file_number назначен двум разным путям".to_owned(),
                });
            }
        } else {
            if self.capacity == 0 {
                return Err(Error::invalid_argument(
                    "table_cache_capacity",
                    "table cache capacity должен быть больше нуля",
                ));
            }
            // Сначала закрываем старый FD, чтобы не превышать budget на reopen.
            if state.opened.len() >= self.capacity {
                let oldest = state
                    .opened
                    .iter()
                    .min_by_key(|(number, handle)| (handle.access, **number))
                    .map(|(number, _)| *number)
                    .ok_or(Error::InternalInvariant {
                        invariant: "I-SST-HANDLE-LRU",
                        reason: "open_handles > 0, но LRU пуст".to_owned(),
                    })?;
                state.opened.remove(&oldest);
                state.evictions = state.evictions.saturating_add(1);
            }
            let actual_size = NativeFileSystem.file_size(path)?;
            if actual_size != metadata.file_size {
                return Err(Error::corruption(
                    "SSTable handle reopen",
                    "физический SST размер не совпадает с проверенным MANIFEST",
                ));
            }
            let mut file = NativeFileSystem.open_random_access(path)?;
            let mut header = [0_u8; SST_HEADER_SIZE];
            read_exact_at(&mut file, path, 0, &mut header, "reopen_sstable_header")?;
            let decoded = decode_header(&header, identity, number)?;
            if decoded.smallest_sequence != metadata.smallest_sequence
                || decoded.largest_sequence != metadata.largest_sequence
            {
                return Err(Error::corruption(
                    "SSTable handle reopen",
                    "последовательности в header не совпадают с MANIFEST",
                ));
            }
            if state.seen.contains(&number) {
                state.reopens = state.reopens.saturating_add(1);
            } else {
                state.seen.insert(number);
            }
            state.misses = state.misses.saturating_add(1);
            let access = state.tick();
            state.opened.insert(
                number,
                CachedHandle {
                    path: path.to_path_buf(),
                    file,
                    access,
                },
            );
        }

        // Счётчик hit учитывает только повторное использование уже открытого FD.
        // A successful first open is miss, not hit.
        if let Some(entry) = state.opened.get(&number)
            && entry.access != state.clock
        {
            state.hits = state.hits.saturating_add(1);
        }
        let access = state.tick();
        let entry = state.opened.get_mut(&number).ok_or(Error::InternalInvariant {
            invariant: "I-SST-HANDLE-LRU",
            reason: "открытый reader отсутствует в pool".to_owned(),
        })?;
        entry.access = access;
        read(&mut entry.file)
    }

    /// При удалении retired SST закрыть cached handle ДО Windows remove_file.
    /// Вызывающий код проверил, что TableReader больше не удерживается snapshot.
    pub fn evict_table(&self, file_number: u64) -> Result<()> {
        let mut state = self.lock()?;
        if state.opened.remove(&file_number).is_some() {
            state.evictions = state.evictions.saturating_add(1);
        }
        Ok(())
    }
}
