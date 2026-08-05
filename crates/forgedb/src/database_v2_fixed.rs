use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use forgedb_core::{DatabaseId, Error, Limits, Result, SequenceNumber, WriteBatch};
use forgedb_fs::{
    FileSystem, IDENTITY_FILE_NAME, NativeFileSystem, NativeLockGuard, load_or_create_identity,
    read_identity,
};
use forgedb_manifest::{
    CURRENT_FILE_NAME, ManifestState, ManifestWriter, VersionEdit, create_manifest, open_manifest,
};
use forgedb_sstable::{
    TableBuildOptions, TableEntry, TableLookup, TableMeta, TableReader, build_table, sst_file_name,
    temporary_sst_file_name,
};
use forgedb_wal::{
    WalBatch, WalWriter, create_wal, open_wal_numbered, replay_wal_numbered, wal_file_name,
};

use crate::memtable::{MemLookup, MemTable};
use crate::{Durability, Options, WriteOptions};

const LOCK_FILE_NAME: &str = "LOCK";
const TEMP_DIRECTORY_NAME: &str = "TEMP";

#[derive(Debug)]
struct PendingFlush {
    memtable: MemTable,
    old_log_number: u64,
    table_file_number: u64,
}

#[derive(Debug)]
struct DatabaseState {
    mutable: MemTable,
    pending_flush: Option<PendingFlush>,
    flush_in_progress: bool,
    wal: Option<WalWriter<File>>,
    manifest_writer: Option<ManifestWriter<File>>,
    manifest: ManifestState,
    l0_tables: Vec<TableReader>,
    next_sequence: SequenceNumber,
    fatal_write_error: Option<String>,
    lock: Option<NativeLockGuard>,
}

#[derive(Debug)]
enum FlushWork {
    Empty {
        old_log_number: u64,
        table_file_number: u64,
    },
    Build {
        entries: Vec<TableEntry>,
        old_log_number: u64,
        table_file_number: u64,
    },
}

/// Встраиваемая локальная key-value база ForgeDB.
#[derive(Debug)]
pub struct Database {
    directory: PathBuf,
    identity: DatabaseId,
    options: Options,
    state: Mutex<DatabaseState>,
    closed: AtomicBool,
}

impl Database {
    /// Открывает базу, восстанавливает MANIFEST, SSTables и необходимые WAL-поколения.
    pub fn open(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        options.validate()?;
        let directory = path.as_ref().to_path_buf();
        let directory_existed = directory.exists();
        let identity_path = directory.join(IDENTITY_FILE_NAME);
        let identity_existed = identity_path.exists();

        if (!directory_existed || !identity_existed) && !options.create_if_missing {
            return Err(Error::invalid_argument(
                "path",
                "база не существует и create_if_missing=false",
            ));
        }
        if options.error_if_exists && identity_existed {
            return Err(Error::invalid_argument(
                "path",
                "база уже существует и error_if_exists=true",
            ));
        }
        if options.read_only && !identity_existed {
            return Err(Error::invalid_argument(
                "read_only",
                "read-only открытие не создаёт новую базу",
            ));
        }

        let filesystem = NativeFileSystem;
        filesystem.create_dir_all(&directory)?;
        let lock = filesystem.lock_exclusive(&directory.join(LOCK_FILE_NAME))?;
        let identity = if options.read_only {
            read_identity(&filesystem, &directory)?
        } else {
            load_or_create_identity(&filesystem, &directory)?
        };
        let limits = options.limits()?;

        let current_exists = directory.join(CURRENT_FILE_NAME).exists();
        let (manifest_writer, manifest, _) = if current_exists {
            let opened = open_manifest(
                &filesystem,
                &directory,
                identity,
                &limits,
                options.read_only,
            )?;
            (opened.writer, opened.state, opened.replay)
        } else if options.read_only {
            return Err(Error::UnsupportedFormat {
                component: "CURRENT/MANIFEST",
                found: 0,
                minimum_supported: 1,
                maximum_supported: 1,
            });
        } else {
            let (writer, state) = create_manifest(&filesystem, &directory, identity, 1, &limits)?;
            (Some(writer), state, Default::default())
        };

        let mut pending_flush = None;
        let mut recovered_last = manifest.last_sequence;
        if let Some(previous_log_number) = manifest.previous_log_number {
            let table_file_number = pending_table_file_number(&manifest)?;
            let mut immutable = MemTable::default();
            let previous_path = directory.join(wal_file_name(previous_log_number));
            let summary = replay_wal_numbered(
                &filesystem,
                &previous_path,
                identity,
                previous_log_number,
                &limits,
                |batch| apply_replayed_batch(&mut immutable, batch, options.memtable_size),
            )?;
            recovered_last = maximum_sequence(recovered_last, summary.last_sequence);
            pending_flush = Some(PendingFlush {
                memtable: immutable,
                old_log_number: previous_log_number,
                table_file_number,
            });
        }

        let mut mutable = MemTable::default();
        let current_path = directory.join(wal_file_name(manifest.log_number));
        if current_exists && !current_path.exists() {
            return Err(Error::corruption(
                "active WAL",
                "MANIFEST ссылается на отсутствующий current WAL",
            ));
        }

        let (wal, current_summary) = if options.read_only {
            let summary = replay_wal_numbered(
                &filesystem,
                &current_path,
                identity,
                manifest.log_number,
                &limits,
                |batch| apply_replayed_batch(&mut mutable, batch, options.memtable_size),
            )?;
            (None, summary)
        } else {
            let (writer, summary) = open_wal_numbered(
                &filesystem,
                &directory,
                identity,
                manifest.log_number,
                &limits,
                |batch| apply_replayed_batch(&mut mutable, batch, options.memtable_size),
            )?;
            (Some(writer), summary)
        };
        recovered_last = maximum_sequence(recovered_last, current_summary.last_sequence);

        let mut l0_tables = Vec::with_capacity(manifest.l0_files.len());
        for metadata in &manifest.l0_files {
            l0_tables.push(TableReader::open(
                &filesystem,
                &directory,
                identity,
                metadata,
                &limits,
                options.max_compaction_bytes,
            )?);
        }
        let next_sequence = match recovered_last {
            Some(last) => last.checked_next()?,
            None => SequenceNumber::FIRST,
        };

        Ok(Self {
            directory,
            identity,
            options,
            state: Mutex::new(DatabaseState {
                mutable,
                pending_flush,
                flush_in_progress: false,
                wal,
                manifest_writer,
                manifest,
                l0_tables,
                next_sequence,
                fatal_write_error: None,
                lock: Some(lock),
            }),
            closed: AtomicBool::new(false),
        })
    }

    /// Записывает или заменяет значение одного ключа.
    pub fn put(
        &self,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        write_options: WriteOptions,
    ) -> Result<()> {
        let mut batch = WriteBatch::with_limits(self.options.limits()?);
        batch.put(key, value)?;
        self.write(batch, write_options)
    }

    /// Добавляет tombstone для одного ключа.
    pub fn delete(&self, key: impl AsRef<[u8]>, write_options: WriteOptions) -> Result<()> {
        let mut batch = WriteBatch::with_limits(self.options.limits()?);
        batch.delete(key)?;
        self.write(batch, write_options)
    }

    /// Применяет WriteBatch атомарно относительно читателей и recovery.
    pub fn write(&self, batch: WriteBatch, write_options: WriteOptions) -> Result<()> {
        self.ensure_open()?;
        if self.options.read_only {
            return Err(Error::ReadOnly);
        }
        let limits = self.options.limits()?;
        batch.validate_against(&limits)?;
        if batch.is_empty() {
            return Err(Error::invalid_argument(
                "batch",
                "batch не должен быть пустым",
            ));
        }

        let mut state = self.lock_state()?;
        ensure_writer_healthy(&state)?;
        let sequence_start = state.next_sequence;
        let sequence_end = sequence_start.inclusive_end(batch.len())?;
        let next_sequence = sequence_end.checked_next()?;
        let prepared = state
            .mutable
            .prepare(sequence_start, &batch, self.options.memtable_size)?;

        let append_result = match write_options.durability() {
            Durability::Sync => state
                .wal
                .as_mut()
                .ok_or(Error::InternalInvariant {
                    invariant: "I-WAL-WRITER-PRESENT",
                    reason: "write path не содержит WAL writer".to_owned(),
                })?
                .append_batch(sequence_start, &batch, &limits, true)
                .map(|_| ()),
            Durability::NoSync => state
                .wal
                .as_mut()
                .ok_or(Error::InternalInvariant {
                    invariant: "I-WAL-WRITER-PRESENT",
                    reason: "write path не содержит WAL writer".to_owned(),
                })?
                .append_batch(sequence_start, &batch, &limits, false)
                .map(|_| ()),
            Durability::DisableWal => Ok(()),
        };
        if let Err(error) = append_result {
            state.fatal_write_error = Some(error.to_string());
            return Err(error);
        }

        state.mutable.apply(prepared);
        state.next_sequence = next_sequence;
        Ok(())
    }

    /// Возвращает последнее видимое значение из mutable, immutable и L0 от новых к старым.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        self.ensure_open()?;
        let key = key.as_ref();
        self.options.limits()?.validate_key(key)?;
        let state = self.lock_state()?;
        match state.mutable.get(key) {
            MemLookup::Value(value) => return Ok(Some(value)),
            MemLookup::Tombstone => return Ok(None),
            MemLookup::Missing => {}
        }
        if let Some(pending) = &state.pending_flush {
            match pending.memtable.get(key) {
                MemLookup::Value(value) => return Ok(Some(value)),
                MemLookup::Tombstone => return Ok(None),
                MemLookup::Missing => {}
            }
        }
        for table in state.l0_tables.iter().rev() {
            match table.get(key)? {
                TableLookup::Value(value) => return Ok(Some(value)),
                TableLookup::Tombstone => return Ok(None),
                TableLookup::Missing => {}
            }
        }
        Ok(None)
    }

    /// Синхронно переводит immutable memtable в установленный L0 SSTable.
    pub fn flush(&self) -> Result<()> {
        self.ensure_open()?;
        if self.options.read_only {
            return Err(Error::ReadOnly);
        }
        let limits = self.options.limits()?;
        let work = self.prepare_flush(&limits)?;
        let Some(work) = work else {
            return Ok(());
        };

        let result = match work {
            FlushWork::Empty {
                old_log_number,
                table_file_number,
            } => self.install_empty_flush(old_log_number, table_file_number, &limits),
            FlushWork::Build {
                entries,
                old_log_number,
                table_file_number,
            } => self.build_and_install_flush(entries, old_log_number, table_file_number, &limits),
        };
        if result.is_err() {
            self.clear_flush_in_progress();
        }
        result
    }

    /// Синхронизирует WAL и MANIFEST и закрывает базу для последующих вызовов API.
    pub fn close(&self) -> Result<()> {
        let mut state = self.lock_state_allow_closed()?;
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        if state.flush_in_progress {
            return Err(Error::Busy {
                reason: "flush ещё выполняется".to_owned(),
            });
        }
        if let Some(writer) = state.wal.as_ref() {
            writer.sync()?;
        }
        if let Some(writer) = state.manifest_writer.as_ref() {
            writer.sync()?;
        }
        state.wal.take();
        state.manifest_writer.take();
        state.lock.take();
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Возвращает стабильный идентификатор базы.
    #[must_use]
    pub const fn identity(&self) -> DatabaseId {
        self.identity
    }

    /// Возвращает путь открытого каталога.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.directory
    }

    fn prepare_flush(&self, limits: &Limits) -> Result<Option<FlushWork>> {
        let filesystem = NativeFileSystem;
        let mut state = self.lock_state()?;
        ensure_writer_healthy(&state)?;
        if state.flush_in_progress {
            return Err(Error::Busy {
                reason: "flush уже выполняется".to_owned(),
            });
        }

        if state.pending_flush.is_none() {
            if state.mutable.is_empty() {
                return Ok(None);
            }
            let wal_sync_result = state
                .wal
                .as_ref()
                .ok_or(Error::InternalInvariant {
                    invariant: "I-WAL-WRITER-PRESENT",
                    reason: "flush не содержит current WAL writer".to_owned(),
                })?
                .sync();
            if let Err(error) = wal_sync_result {
                state.fatal_write_error = Some(error.to_string());
                return Err(error);
            }

            let table_file_number = state.manifest.next_file_number;
            let new_log_number = table_file_number
                .checked_add(1)
                .ok_or(Error::ResourceLimit {
                    resource: "file_number",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?;
            let next_file_number = new_log_number.checked_add(1).ok_or(Error::ResourceLimit {
                resource: "file_number",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
            let old_log_number = state.manifest.log_number;
            let new_wal = create_wal(&filesystem, &self.directory, self.identity, new_log_number)?;
            let rotation = VersionEdit {
                log_number: Some(new_log_number),
                previous_log_number: Some(Some(old_log_number)),
                next_file_number: Some(next_file_number),
                last_sequence: Some(state.mutable.maximum_sequence()),
                ..VersionEdit::default()
            };
            append_manifest_edit_or_poison(&mut state, &rotation, limits)?;
            let frozen = std::mem::take(&mut state.mutable);
            state.pending_flush = Some(PendingFlush {
                memtable: frozen,
                old_log_number,
                table_file_number,
            });
            state.wal = Some(new_wal);
        }

        let pending = state
            .pending_flush
            .as_ref()
            .ok_or(Error::InternalInvariant {
                invariant: "I-PENDING-FLUSH-PRESENT",
                reason: "flush не содержит immutable memtable".to_owned(),
            })?;
        let work = if pending.memtable.is_empty() {
            FlushWork::Empty {
                old_log_number: pending.old_log_number,
                table_file_number: pending.table_file_number,
            }
        } else {
            FlushWork::Build {
                entries: pending.memtable.table_entries(limits)?,
                old_log_number: pending.old_log_number,
                table_file_number: pending.table_file_number,
            }
        };
        state.flush_in_progress = true;
        Ok(Some(work))
    }

    fn build_and_install_flush(
        &self,
        entries: Vec<TableEntry>,
        old_log_number: u64,
        table_file_number: u64,
        limits: &Limits,
    ) -> Result<()> {
        let filesystem = NativeFileSystem;
        self.remove_reserved_sst(table_file_number)?;
        let metadata = build_table(
            &filesystem,
            forgedb_sstable::TableBuildRequest::new(
                &self.directory,
                &self.directory.join(TEMP_DIRECTORY_NAME),
                self.identity,
                table_file_number,
                &entries,
                limits,
                TableBuildOptions {
                    data_block_size: self.options.data_block_size,
                    restart_interval: self.options.restart_interval,
                    bloom_bits_per_key: self.options.bloom_bits_per_key,
                },
            ),
        )?;
        let reader = TableReader::open(
            &filesystem,
            &self.directory,
            self.identity,
            &metadata,
            limits,
            self.options.max_compaction_bytes,
        )?;
        self.install_flush(metadata, reader, old_log_number, table_file_number, limits)
    }

    fn install_flush(
        &self,
        metadata: TableMeta,
        reader: TableReader,
        old_log_number: u64,
        table_file_number: u64,
        limits: &Limits,
    ) -> Result<()> {
        {
            let mut state = self.lock_state()?;
            validate_pending_generation(&state, old_log_number, table_file_number)?;
            let last_sequence = state
                .pending_flush
                .as_ref()
                .and_then(|pending| pending.memtable.maximum_sequence());
            let installation = VersionEdit {
                previous_log_number: Some(None),
                last_sequence: Some(last_sequence),
                added_files: vec![metadata],
                ..VersionEdit::default()
            };
            append_manifest_edit_or_poison(&mut state, &installation, limits)?;
            state.l0_tables.push(reader);
            state.pending_flush = None;
            state.flush_in_progress = false;
        }
        remove_obsolete_wal(&self.directory, old_log_number)
    }

    fn install_empty_flush(
        &self,
        old_log_number: u64,
        table_file_number: u64,
        limits: &Limits,
    ) -> Result<()> {
        self.remove_reserved_sst(table_file_number)?;
        {
            let mut state = self.lock_state()?;
            validate_pending_generation(&state, old_log_number, table_file_number)?;
            let installation = VersionEdit {
                previous_log_number: Some(None),
                ..VersionEdit::default()
            };
            append_manifest_edit_or_poison(&mut state, &installation, limits)?;
            state.pending_flush = None;
            state.flush_in_progress = false;
        }
        remove_obsolete_wal(&self.directory, old_log_number)
    }

    fn remove_reserved_sst(&self, table_file_number: u64) -> Result<()> {
        let filesystem = NativeFileSystem;
        {
            let state = self.lock_state()?;
            if state
                .manifest
                .l0_files
                .iter()
                .any(|file| file.file_number == table_file_number)
            {
                return Err(Error::InternalInvariant {
                    invariant: "I-FLUSH-RESERVED-SST-NOT-LIVE",
                    reason: "reserved SST уже находится в live VersionSet".to_owned(),
                });
            }
        }
        let temporary_directory = self.directory.join(TEMP_DIRECTORY_NAME);
        let temporary = temporary_directory.join(temporary_sst_file_name(table_file_number));
        let final_path = self.directory.join(sst_file_name(table_file_number));
        let removed_temp = remove_if_exists(&filesystem, &temporary)?;
        let removed_final = remove_if_exists(&filesystem, &final_path)?;
        if removed_temp {
            filesystem.sync_directory(&temporary_directory)?;
        }
        if removed_final {
            filesystem.sync_directory(&self.directory)?;
        }
        Ok(())
    }

    fn clear_flush_in_progress(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.flush_in_progress = false;
        }
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, DatabaseState>> {
        self.ensure_open()?;
        let state = self.lock_state_allow_closed()?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        Ok(state)
    }

    fn lock_state_allow_closed(&self) -> Result<MutexGuard<'_, DatabaseState>> {
        self.state.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-DATABASE-MUTEX",
            reason: "mutex состояния базы poisoned".to_owned(),
        })
    }
}

fn ensure_writer_healthy(state: &DatabaseState) -> Result<()> {
    if let Some(reason) = &state.fatal_write_error {
        return Err(Error::BackgroundFailure {
            component: "storage writer",
            reason: reason.clone(),
        });
    }
    Ok(())
}

fn pending_table_file_number(manifest: &ManifestState) -> Result<u64> {
    let table_file_number = manifest
        .log_number
        .checked_sub(1)
        .filter(|number| *number != 0)
        .ok_or_else(|| {
            Error::corruption(
                "MANIFEST flush generation",
                "current WAL не позволяет восстановить reserved SST number",
            )
        })?;
    if manifest
        .l0_files
        .iter()
        .any(|file| file.file_number == table_file_number)
    {
        return Err(Error::corruption(
            "MANIFEST flush generation",
            "reserved SST number уже установлен в L0",
        ));
    }
    Ok(table_file_number)
}

fn validate_pending_generation(
    state: &DatabaseState,
    old_log_number: u64,
    table_file_number: u64,
) -> Result<()> {
    let pending = state
        .pending_flush
        .as_ref()
        .ok_or(Error::InternalInvariant {
            invariant: "I-PENDING-FLUSH-PRESENT",
            reason: "immutable memtable исчезла до installation".to_owned(),
        })?;
    if pending.table_file_number != table_file_number
        || pending.old_log_number != old_log_number
        || state.manifest.previous_log_number != Some(old_log_number)
    {
        return Err(Error::InternalInvariant {
            invariant: "I-FLUSH-GENERATION-STABLE",
            reason: "поколение flush изменилось во время построения SSTable".to_owned(),
        });
    }
    Ok(())
}

fn append_manifest_edit(
    state: &mut DatabaseState,
    edit: &VersionEdit,
    limits: &Limits,
) -> Result<()> {
    state
        .manifest_writer
        .as_mut()
        .ok_or(Error::InternalInvariant {
            invariant: "I-MANIFEST-WRITER-PRESENT",
            reason: "read-write база не содержит MANIFEST writer".to_owned(),
        })?
        .append_edit(&mut state.manifest, edit, limits)
}

fn append_manifest_edit_or_poison(
    state: &mut DatabaseState,
    edit: &VersionEdit,
    limits: &Limits,
) -> Result<()> {
    if let Err(error) = append_manifest_edit(state, edit, limits) {
        state.fatal_write_error = Some(error.to_string());
        return Err(error);
    }
    Ok(())
}

fn apply_replayed_batch(
    memtable: &mut MemTable,
    wal_batch: WalBatch,
    memory_limit: usize,
) -> Result<()> {
    let prepared = memtable.prepare(wal_batch.sequence_start, &wal_batch.batch, memory_limit)?;
    memtable.apply(prepared);
    Ok(())
}

fn maximum_sequence(
    left: Option<SequenceNumber>,
    right: Option<SequenceNumber>,
) -> Option<SequenceNumber> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn remove_if_exists(filesystem: &NativeFileSystem, path: &Path) -> Result<bool> {
    match filesystem.remove_file(path) {
        Ok(()) => Ok(true),
        Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn remove_obsolete_wal(directory: &Path, old_log_number: u64) -> Result<()> {
    let filesystem = NativeFileSystem;
    let obsolete = directory.join(wal_file_name(old_log_number));
    if remove_if_exists(&filesystem, &obsolete)? {
        filesystem.sync_directory(directory)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("forgedb-db-{name}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn sync_put_delete_and_batch_survive_reopen() -> Result<()> {
        let directory = temporary_directory("reopen")?;
        let db = Database::open(&directory, Options::default())?;
        db.put(b"a", b"one", WriteOptions::sync())?;
        let mut batch = WriteBatch::new();
        batch.put(b"b", b"two")?;
        batch.delete(b"a")?;
        db.write(batch, WriteOptions::sync())?;
        db.close()?;

        let reopened = Database::open(&directory, Options::default())?;
        assert_eq!(reopened.get(b"a")?, None);
        assert_eq!(reopened.get(b"b")?, Some(b"two".to_vec()));
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn flush_installs_sstable_and_reopen_reads_it() -> Result<()> {
        let directory = temporary_directory("flush")?;
        let db = Database::open(&directory, Options::default())?;
        db.put(b"alpha", b"one", WriteOptions::sync())?;
        db.put(b"beta", b"two", WriteOptions::sync())?;
        db.delete(b"beta", WriteOptions::sync())?;
        db.flush()?;
        assert_eq!(db.get(b"alpha")?, Some(b"one".to_vec()));
        assert_eq!(db.get(b"beta")?, None);
        db.close()?;

        let reopened = Database::open(&directory, Options::default())?;
        assert_eq!(reopened.get(b"alpha")?, Some(b"one".to_vec()));
        assert_eq!(reopened.get(b"beta")?, None);
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn writes_after_flush_override_l0() -> Result<()> {
        let directory = temporary_directory("override")?;
        let db = Database::open(&directory, Options::default())?;
        db.put(b"key", b"old", WriteOptions::sync())?;
        db.flush()?;
        db.put(b"key", b"new", WriteOptions::sync())?;
        assert_eq!(db.get(b"key")?, Some(b"new".to_vec()));
        db.close()?;

        let reopened = Database::open(&directory, Options::default())?;
        assert_eq!(reopened.get(b"key")?, Some(b"new".to_vec()));
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn deterministic_model_matches_after_flush_and_reopen() -> Result<()> {
        let directory = temporary_directory("model")?;
        let db = Database::open(&directory, Options::default())?;
        let mut model = BTreeMap::<Vec<u8>, Option<Vec<u8>>>::new();
        let mut state = 0x4d59_5df4_d0f3_3173_u64;
        for step in 0..500_u64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let key = format!("key-{}", state % 37).into_bytes();
            if state & 3 == 0 {
                db.delete(&key, WriteOptions::sync())?;
                model.insert(key, None);
            } else {
                let value = format!("value-{step}-{}", state >> 16).into_bytes();
                db.put(&key, &value, WriteOptions::sync())?;
                model.insert(key, Some(value));
            }
            if step == 249 {
                db.flush()?;
            }
        }
        db.flush()?;
        db.close()?;

        let reopened = Database::open(&directory, Options::default())?;
        for (key, expected) in model {
            assert_eq!(reopened.get(&key)?, expected);
        }
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn recovery_keeps_previous_and_current_wal_in_separate_generations() -> Result<()> {
        let directory = temporary_directory("rotated-recovery")?;
        let filesystem = NativeFileSystem;
        let options = Options::default();
        let limits = options.limits()?;
        let identity = load_or_create_identity(&filesystem, &directory)?;
        let (mut manifest_writer, mut manifest) =
            create_manifest(&filesystem, &directory, identity, 1, &limits)?;

        let mut previous_wal = create_wal(&filesystem, &directory, identity, 1)?;
        let mut previous_batch = WriteBatch::new();
        previous_batch.put(b"previous", b"durable")?;
        previous_wal.append_batch(SequenceNumber::FIRST, &previous_batch, &limits, true)?;

        let table_file_number = manifest.next_file_number;
        let current_log_number = table_file_number + 1;
        let next_file_number = current_log_number + 1;
        let current_wal = create_wal(&filesystem, &directory, identity, current_log_number)?;
        manifest_writer.append_edit(
            &mut manifest,
            &VersionEdit {
                log_number: Some(current_log_number),
                previous_log_number: Some(Some(1)),
                next_file_number: Some(next_file_number),
                last_sequence: Some(Some(SequenceNumber::FIRST)),
                ..VersionEdit::default()
            },
            &limits,
        )?;
        drop(previous_wal);
        drop(current_wal);
        drop(manifest_writer);

        let db = Database::open(&directory, options)?;
        assert_eq!(db.get(b"previous")?, Some(b"durable".to_vec()));
        db.put(b"current", b"newer", WriteOptions::sync())?;
        db.flush()?;
        assert!(directory.join(sst_file_name(table_file_number)).exists());
        assert!(!directory.join(wal_file_name(1)).exists());
        assert_eq!(db.get(b"current")?, Some(b"newer".to_vec()));
        db.flush()?;
        db.close()?;

        let reopened = Database::open(&directory, Options::default())?;
        assert_eq!(reopened.get(b"previous")?, Some(b"durable".to_vec()));
        assert_eq!(reopened.get(b"current")?, Some(b"newer".to_vec()));
        reopened.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn missing_active_wal_is_always_corruption() -> Result<()> {
        let directory = temporary_directory("missing-wal")?;
        let db = Database::open(&directory, Options::default())?;
        db.put(b"key", b"value", WriteOptions::sync())?;
        db.close()?;
        fs::remove_file(directory.join(wal_file_name(1)))
            .map_err(|source| Error::io("remove_test_wal", Some(directory.clone()), source))?;
        assert!(matches!(
            Database::open(&directory, Options::default()),
            Err(Error::Corruption { .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
