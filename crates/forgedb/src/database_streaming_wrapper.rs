/// Публичный Database wrapper с block-at-a-time SSTable scan.
#[derive(Debug)]
pub struct StreamingDatabase {
    inner: Database,
}

impl StreamingDatabase {
    /// Открывает базу и восстанавливает WAL/MANIFEST/VersionSet через основной storage core.
    pub fn open(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        Database::open(path, options).map(|inner| Self { inner })
    }

    /// Записывает или заменяет значение одного ключа.
    pub fn put(
        &self,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        write_options: WriteOptions,
    ) -> Result<()> {
        self.inner.put(key, value, write_options)
    }

    /// Добавляет MVCC tombstone.
    pub fn delete(&self, key: impl AsRef<[u8]>, write_options: WriteOptions) -> Result<()> {
        self.inner.delete(key, write_options)
    }

    /// Применяет WriteBatch атомарно после выбранной WAL durability boundary.
    pub fn write(&self, batch: WriteBatch, write_options: WriteOptions) -> Result<()> {
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
        DbIterator::new_cursors(sources, snapshot, owned_range, read_options.limit)
    }

    /// Синхронно переводит immutable memtable в установленный L0 SSTable.
    pub fn flush(&self) -> Result<()> {
        self.inner.flush()
    }

    /// Возвращает неразрушающий file inventory.
    pub fn file_inventory(&self) -> Result<FileInventory> {
        self.inner.file_inventory()
    }

    /// Выполняет quick или deep verification live state.
    pub fn verify(&self, verify_options: VerifyOptions) -> Result<VerifyReport> {
        self.inner.verify(verify_options)
    }

    /// Синхронизирует writers и освобождает process lock.
    pub fn close(&self) -> Result<()> {
        self.inner.close()
    }

    /// Возвращает persisted UUID базы.
    #[must_use]
    pub const fn identity(&self) -> DatabaseId {
        self.inner.identity()
    }

    /// Возвращает путь открытого каталога.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.inner.path()
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
    fn public_database_scan_uses_multiple_sstable_blocks() -> Result<()> {
        let directory = temporary_directory("range")?;
        let mut options = Options::default();
        options.data_block_size = 1024;
        let database = StreamingDatabase::open(&directory, options)?;
        for index in 0..160_u64 {
            database.put(
                format!("key-{index:04}").as_bytes(),
                vec![u8::try_from(index % 251).unwrap_or(0); 256],
                WriteOptions::sync(),
            )?;
        }
        database.flush()?;
        let rows = database
            .scan(.., ReadOptions::default())?
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(rows.len(), 160);
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
