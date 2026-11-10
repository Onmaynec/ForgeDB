/// Публичный Database wrapper с block-at-a-time SSTable scan и единым public write gate.
#[derive(Debug)]
pub struct StreamingDatabase {
    inner: Arc<Database>,
    write_gate: Arc<Mutex<()>>,
}

impl StreamingDatabase {
    /// Открывает базу и восстанавливает WAL/MANIFEST/VersionSet через основной storage core.
    pub fn open(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        Database::open(path, options).map(|inner| Self {
            inner: Arc::new(inner),
            write_gate: Arc::new(Mutex::new(())),
        })
    }

    /// Записывает или заменяет значение одного ключа.
    pub fn put(
        &self,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        write_options: WriteOptions,
    ) -> Result<()> {
        let _write_guard = self.lock_write_gate()?;
        self.apply_write_stall()?;
        self.inner.put(key, value, write_options)
    }

    /// Добавляет MVCC tombstone.
    pub fn delete(&self, key: impl AsRef<[u8]>, write_options: WriteOptions) -> Result<()> {
        let _write_guard = self.lock_write_gate()?;
        self.apply_write_stall()?;
        self.inner.delete(key, write_options)
    }

    /// Применяет WriteBatch атомарно после выбранной WAL durability boundary.
    pub fn write(&self, batch: WriteBatch, write_options: WriteOptions) -> Result<()> {
        let _write_guard = self.lock_write_gate()?;
        self.apply_write_stall()?;
        self.inner.write(batch, write_options)
    }

    /// Возвращает latest visible value.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        self.inner.get(key)
    }

    /// Выполняет snapshot-aware point lookup.
    pub fn get_with_options(
        &self,
        key: impl AsRef<[u8]>,
        read_options: ReadOptions,
    ) -> Result<Option<Vec<u8>>> {
        self.inner.get_with_options(key, read_options)
    }

    /// Фиксирует latest visible sequence.
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.inner.snapshot()
    }

    /// Начинает optimistic transaction на snapshot текущего visible sequence.
    pub fn begin_transaction(&self) -> Result<crate::Transaction> {
        crate::Transaction::begin(self)
    }

    /// Создаёт forward iterator; SSTable sources держат index и один data block.
    pub fn scan<R>(&self, range: R, read_options: ReadOptions) -> Result<DbIterator>
    where
        R: RangeBounds<Vec<u8>>,
    {
        self.inner.ensure_open()?;
        let owned_range = OwnedRange::from_range(&range)?;
        let limits = self.inner.options.limits()?;
        let (snapshot, version, mut sources) = {
            let state = self.inner.lock_state()?;
            let snapshot = self.inner.resolve_read_sequence(&state, &read_options)?;
            let mut sources: Vec<(
                usize,
                Box<dyn crate::entry_cursor::EntryCursor>,
            )> = vec![(
                0,
                Box::new(crate::entry_cursor::VecEntryCursor::new(
                    state.mutable.table_entries(),
                )),
            )];
            if let Some(pending) = &state.pending_flush {
                sources.push((
                    1,
                    Box::new(crate::entry_cursor::VecEntryCursor::new(
                        pending.memtable.table_entries(),
                    )),
                ));
            }
            (snapshot, Arc::clone(&state.version), sources)
        };
        if snapshot.is_none() {
            return Ok(DbIterator::empty(snapshot, read_options.limit));
        }

        let mut priority = sources.len();
        for table in version.files_in_read_order()? {
            if !owned_range.overlaps(table.metadata()) {
                continue;
            }
            let cursor = crate::sst_entry_cursor::SstEntryCursor::open(
                table.path(),
                self.inner.identity,
                table.metadata(),
                &limits,
                self.inner.options.max_compaction_bytes,
                read_options.verify_checksums,
            )?;
            sources.push((priority, Box::new(cursor)));
            priority = priority.checked_add(1).ok_or(Error::ResourceLimit {
                resource: "scan_source_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        }
        Ok(DbIterator::new_cursors(sources, snapshot, owned_range, read_options.limit)?
            .with_version_lease(version))
    }

    /// Сохраняет compile-ссылку на materialized scan до объединения wrapper и storage core.
    #[allow(dead_code)]
    fn legacy_materialized_scan<R>(
        &self,
        range: R,
        read_options: ReadOptions,
    ) -> Result<DbIterator>
    where
        R: RangeBounds<Vec<u8>>,
    {
        self.inner.scan(range, read_options)
    }

    /// Синхронно переводит immutable memtable в установленный L0 SSTable и при необходимости
    /// выполняет synchronous leveled compaction до снижения compaction pressure.
    pub fn flush(&self) -> Result<()> {
        self.inner.flush()?;
        self.compact_if_needed()
    }

    /// Возвращает неразрушающий file inventory.
    pub fn file_inventory(&self) -> Result<FileInventory> {
        self.inner.file_inventory()
    }

    /// Выполняет quick или deep verification live state.
    pub fn verify(&self, verify_options: VerifyOptions) -> Result<VerifyReport> {
        self.inner.verify(verify_options)
    }

    /// Текущие hits/misses/evictions и residency bounded SST block cache.
    pub fn cache_stats(&self) -> Result<CacheStats> {
        self.inner.cache_stats()
    }

    /// Синхронизирует writers и освобождает process lock.
    pub fn close(&self) -> Result<()> {
        self.inner.close()
    }

    /// Возвращает persisted UUID базы.
    #[must_use]
    pub fn identity(&self) -> DatabaseId {
        self.inner.identity()
    }

    /// Возвращает путь открытого каталога.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.inner.path()
    }

    pub(crate) fn transaction_handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            write_gate: Arc::clone(&self.write_gate),
        }
    }

    pub(crate) fn new_transaction_batch(&self) -> Result<(WriteBatch, usize, usize)> {
        let limits = self.inner.options.limits()?;
        Ok((
            WriteBatch::with_limits(limits),
            limits.max_batch_operations,
            limits.max_batch_size,
        ))
    }

    pub(crate) fn validate_transaction_key(&self, key: &[u8]) -> Result<()> {
        self.inner.options.limits()?.validate_key(key)
    }

    pub(crate) fn commit_transaction(
        &self,
        snapshot: &Snapshot,
        batch: WriteBatch,
        write_options: WriteOptions,
    ) -> Result<()> {
        if write_options.durability() == Durability::DisableWal {
            return Err(Error::invalid_argument(
                "durability",
                "DisableWal запрещён для transaction commit",
            ));
        }
        if !snapshot.belongs_to(self.inner.identity, self.inner.handle_id) {
            return Err(Error::invalid_argument(
                "snapshot",
                "transaction snapshot создан другой базой или другим open handle",
            ));
        }
        let limits = self.inner.options.limits()?;
        batch.validate_against(&limits)?;
        if batch.is_empty() {
            return Ok(());
        }

        // Gate охватывает validation и последующий WAL append: ни один другой public write не
        // может вклиниться между проверкой write set и atomic Database::write.
        let _write_guard = self.lock_write_gate()?;
        self.apply_write_stall()?;
        let snapshot_sequence = snapshot.sequence();
        let threshold = snapshot_sequence.map(SequenceNumber::get).unwrap_or(0);
        let mut write_keys = std::collections::BTreeSet::<Vec<u8>>::new();
        for operation in batch.operations() {
            let key = match operation {
                forgedb_core::BatchOperation::Put { key, .. }
                | forgedb_core::BatchOperation::Delete { key, .. } => key,
            };
            write_keys.insert(key.clone());
        }

        let (version, mut conflicts) = {
            let state = self.inner.lock_state()?;
            ensure_writer_healthy(&state)?;
            let mut conflicts = Vec::new();
            for key in &write_keys {
                let mutable_conflict = state
                    .mutable
                    .latest_sequence(key)
                    .is_some_and(|sequence| sequence.get() > threshold);
                let immutable_conflict = state
                    .pending_flush
                    .as_ref()
                    .and_then(|pending| pending.memtable.latest_sequence(key))
                    .is_some_and(|sequence| sequence.get() > threshold);
                if mutable_conflict || immutable_conflict {
                    conflicts.push(key.clone());
                }
            }
            (Arc::clone(&state.version), conflicts)
        };

        for key in &write_keys {
            if conflicts.binary_search(key).is_ok() {
                continue;
            }
            if version
                .latest_sequence_after(key, snapshot_sequence, &limits, true)?
                .is_some()
            {
                conflicts.push(key.clone());
                conflicts.sort();
            }
        }
        if !conflicts.is_empty() {
            return Err(Error::Conflict { keys: conflicts });
        }

        self.inner.write(batch, write_options)
    }

    fn lock_write_gate(&self) -> Result<MutexGuard<'_, ()>> {
        self.write_gate.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-PUBLIC-WRITE-GATE",
            reason: "public write gate mutex poisoned".to_owned(),
        })
    }
}

#[cfg(test)]
mod streaming_wrapper_tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-streaming-db-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn public_database_scan_streams_multiple_sstables() -> Result<()> {
        let directory = temporary_directory("multi-file-range")?;
        let options = Options {
            data_block_size: 1024,
            l0_compaction_trigger: 8,
            l0_slowdown_trigger: 9,
            l0_stop_trigger: 10,
            ..Options::default()
        };
        let database = StreamingDatabase::open(&directory, options)?;

        for generation in 0..3_u64 {
            let mut batch = WriteBatch::new();
            for offset in 0..120_u64 {
                let index = generation
                    .checked_mul(120)
                    .and_then(|base| base.checked_add(offset))
                    .ok_or(Error::ResourceLimit {
                        resource: "test_key_index",
                        limit: u64::MAX,
                        observed: u64::MAX,
                    })?;
                batch.put(
                    format!("key-{index:04}").into_bytes(),
                    vec![u8::try_from(index % 251).unwrap_or(0); 256],
                )?;
            }
            database.write(batch, WriteOptions::sync())?;
            database.flush()?;
        }

        assert_eq!(database.file_inventory()?.live_sst.len(), 3);
        let rows = database
            .scan(
                b"key-0050".to_vec()..b"key-0310".to_vec(),
                ReadOptions::default(),
            )?
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(rows.len(), 260);
        assert!(rows.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(
            rows.first().map(|row| row.0.as_slice()),
            Some(b"key-0050".as_slice())
        );
        assert_eq!(
            rows.last().map(|row| row.0.as_slice()),
            Some(b"key-0309".as_slice())
        );

        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn flush_trigger_compacts_l0_and_reopen_preserves_state() -> Result<()> {
        let directory = temporary_directory("leveled-reopen")?;
        let options = Options {
            l0_compaction_trigger: 2,
            l0_slowdown_trigger: 3,
            l0_stop_trigger: 4,
            target_sst_size: 1024 * 1024,
            ..Options::default()
        };
        let database = StreamingDatabase::open(&directory, options.clone())?;
        database.put(b"a", b"one", WriteOptions::sync())?;
        database.flush()?;
        database.put(b"b", b"two", WriteOptions::sync())?;
        database.flush()?;
        assert_eq!(database.get(b"a")?, Some(b"one".to_vec()));
        assert_eq!(database.get(b"b")?, Some(b"two".to_vec()));
        database.close()?;

        let reopened = StreamingDatabase::open(&directory, options)?;
        assert_eq!(reopened.get(b"a")?, Some(b"one".to_vec()));
        assert_eq!(reopened.get(b"b")?, Some(b"two".to_vec()));
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
