use std::fs::File;
use std::io::ErrorKind;
use std::ops::RangeBounds;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use forgedb_core::{DatabaseId, Error, Limits, Result, SequenceNumber, WriteBatch};
use forgedb_fs::{
    FileSystem, IDENTITY_FILE_NAME, NativeFileSystem, NativeLockGuard, load_or_create_identity,
    read_identity,
};
use forgedb_manifest::{
    CURRENT_FILE_NAME, ManifestState, ManifestWriter, VersionEdit, create_manifest, open_manifest,
};
use forgedb_sstable::{
    TableBuildOptions, TableBuildRequest, TableReader, build_table, sst_file_name,
};
use forgedb_wal::{
    WalBatch, WalWriter, create_wal, open_wal_numbered, replay_wal_numbered, wal_file_name,
};

use crate::inventory::FileInventory;
use crate::iterator::{DbIterator, OwnedRange};
use crate::mvcc_memtable::{MemLookup, MvccMemTable};
use crate::snapshot::SnapshotRegistry;
use crate::table_scan::{self, TableSnapshotLookup};
use crate::verify::{VerifyOptions, VerifyReport, verify_version};
use crate::version_set::VersionSet;
use crate::{Durability, Options, ReadOptions, Snapshot, WriteOptions};

const LOCK_FILE_NAME: &str = "LOCK";
const TEMP_DIRECTORY_NAME: &str = "TEMP";

#[derive(Debug)]
struct PendingFlush {
    memtable: MvccMemTable,
    old_log_number: u64,
    table_file_number: u64,
}

#[derive(Debug)]
struct DatabaseState {
    mutable: MvccMemTable,
    pending_flush: Option<PendingFlush>,
    flush_in_progress: bool,
    wal: Option<WalWriter<File>>,
    manifest_writer: Option<ManifestWriter<File>>,
    manifest: ManifestState,
    version: Arc<VersionSet>,
    next_sequence: SequenceNumber,
    last_sequence: Option<SequenceNumber>,
    fatal_write_error: Option<String>,
    lock: Option<NativeLockGuard>,
}

#[derive(Debug)]
enum FlushWork {
    Noop,
    EmptyPending {
        old_log_number: u64,
        table_file_number: u64,
    },
    BuildTable {
        entries: Vec<forgedb_sstable::TableEntry>,
        old_log_number: u64,
        table_file_number: u64,
    },
}

/// Встраиваемая локальная key-value база ForgeDB.
#[derive(Debug)]
pub struct Database {
    directory: PathBuf,
    identity: DatabaseId,
    handle_id: DatabaseId,
    options: Options,
    state: Mutex<DatabaseState>,
    snapshots: Arc<SnapshotRegistry>,
    closed: AtomicBool,
}

impl Database {
    /// Открывает базу, восстанавливает immutable VersionSet и оба допустимых WAL-поколения.
    pub fn open(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        options.validate()?;
        let directory = path.as_ref().to_path_buf();
        let existed = directory.exists();
        let identity_path = directory.join(IDENTITY_FILE_NAME);
        let identity_existed = identity_path.exists();

        if !existed && !options.create_if_missing {
            return Err(Error::invalid_argument(
                "path",
                "каталог базы не существует и create_if_missing=false",
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
            let opened =
                open_manifest(&filesystem, &directory, identity, &limits, options.read_only)?;
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

        let mut recovered_last = manifest.last_sequence;
        let pending_flush = recover_previous_wal(
            &filesystem,
            &directory,
            identity,
            &manifest,
            &limits,
            options.memtable_size,
            &mut recovered_last,
        )?;

        let mut mutable = MvccMemTable::default();
        let (wal, current_summary) = if options.read_only {
            let current_path = directory.join(wal_file_name(manifest.log_number));
            let summary = replay_wal_numbered(
                &filesystem,
                &current_path,
                identity,
                manifest.log_number,
                &limits,
                |batch| {
                    apply_replayed_batch(
                        &mut mutable,
                        batch,
                        &limits,
                        options.memtable_size,
                    )
                },
            )?;
            (None, summary)
        } else {
            let (writer, summary) = open_wal_numbered(
                &filesystem,
                &directory,
                identity,
                manifest.log_number,
                &limits,
                |batch| {
                    apply_replayed_batch(
                        &mut mutable,
                        batch,
                        &limits,
                        options.memtable_size,
                    )
                },
            )?;
            (Some(writer), summary)
        };
        recovered_last = maximum_sequence(recovered_last, current_summary.last_sequence);

        let version = Arc::new(VersionSet::open(
            &directory,
            identity,
            &manifest,
            &limits,
            options.level_count,
            options.max_compaction_bytes,
        )?);
        let inventory = inventory_for(version.as_ref(), &manifest)?;
        if !inventory.blockers.is_empty() {
            return Err(Error::corruption(
                "file inventory",
                inventory.blockers.join("; "),
            ));
        }
        let next_sequence = match recovered_last {
            Some(last) => last.checked_next()?,
            None => SequenceNumber::FIRST,
        };

        Ok(Self {
            directory,
            identity,
            handle_id: DatabaseId::random(),
            options,
            state: Mutex::new(DatabaseState {
                mutable,
                pending_flush,
                flush_in_progress: false,
                wal,
                manifest_writer,
                manifest,
                version,
                next_sequence,
                last_sequence: recovered_last,
                fatal_write_error: None,
                lock: Some(lock),
            }),
            snapshots: Arc::new(SnapshotRegistry::default()),
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

    /// Добавляет MVCC tombstone.
    pub fn delete(&self, key: impl AsRef<[u8]>, write_options: WriteOptions) -> Result<()> {
        let mut batch = WriteBatch::with_limits(self.options.limits()?);
        batch.delete(key)?;
        self.write(batch, write_options)
    }

    /// Применяет WriteBatch атомарно после выбранной WAL durability boundary.
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
        let prepared = state.mutable.prepare(
            sequence_start,
            &batch,
            &limits,
            self.options.memtable_size,
        )?;

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
        state.last_sequence = Some(sequence_end);
        Ok(())
    }

    /// Возвращает latest visible value.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        self.get_with_options(key, ReadOptions::default())
    }

    /// Выполняет snapshot-aware point lookup по всем read sources.
    pub fn get_with_options(
        &self,
        key: impl AsRef<[u8]>,
        read_options: ReadOptions,
    ) -> Result<Option<Vec<u8>>> {
        self.ensure_open()?;
        let key = key.as_ref();
        let limits = self.options.limits()?;
        limits.validate_key(key)?;

        let (snapshot, version) = {
            let state = self.lock_state()?;
            let snapshot = self.resolve_read_sequence(&state, &read_options)?;
            match state.mutable.get_at(key, snapshot) {
                MemLookup::Value(value) => return Ok(Some(value)),
                MemLookup::Tombstone => return Ok(None),
                MemLookup::Missing => {}
            }
            if let Some(pending) = &state.pending_flush {
                match pending.memtable.get_at(key, snapshot) {
                    MemLookup::Value(value) => return Ok(Some(value)),
                    MemLookup::Tombstone => return Ok(None),
                    MemLookup::Missing => {}
                }
            }
            (snapshot, Arc::clone(&state.version))
        };

        match version.get_at(
            key,
            snapshot,
            &limits,
            read_options.verify_checksums,
        )? {
            TableSnapshotLookup::Missing | TableSnapshotLookup::Tombstone => Ok(None),
            TableSnapshotLookup::Value(value) => Ok(Some(value)),
        }
    }

    /// Фиксирует latest visible sequence и регистрирует active snapshot.
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.ensure_open()?;
        let sequence = self.lock_state()?.last_sequence;
        self.snapshots
            .register(self.identity, self.handle_id, sequence)
    }

    /// Создаёт forward iterator по user keys в заданном range.
    pub fn scan<R>(&self, range: R, read_options: ReadOptions) -> Result<DbIterator>
    where
        R: RangeBounds<Vec<u8>>,
    {
        self.ensure_open()?;
        let owned_range = OwnedRange::from_range(&range)?;
        let limits = self.options.limits()?;
        let (snapshot, version, mut sources) = {
            let state = self.lock_state()?;
            let snapshot = self.resolve_read_sequence(&state, &read_options)?;
            let mut sources = vec![(0_usize, state.mutable.table_entries())];
            if let Some(pending) = &state.pending_flush {
                sources.push((1, pending.memtable.table_entries()));
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
            let entries = table_scan::read_entries(
                table.path(),
                self.identity,
                table.metadata(),
                &limits,
                self.options.max_compaction_bytes,
                read_options.verify_checksums,
            )?;
            sources.push((priority, entries));
            priority = priority.checked_add(1).ok_or(Error::ResourceLimit {
                resource: "scan_source_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        }
        DbIterator::new(sources, snapshot, owned_range, read_options.limit)
    }

    /// Синхронно переводит immutable memtable в установленный L0 SSTable.
    pub fn flush(&self) -> Result<()> {
        self.ensure_open()?;
        if self.options.read_only {
            return Err(Error::ReadOnly);
        }
        let filesystem = NativeFileSystem;
        let limits = self.options.limits()?;
        let work = self.prepare_flush(&filesystem, &limits)?;

        match work {
            FlushWork::Noop => Ok(()),
            FlushWork::EmptyPending {
                old_log_number,
                table_file_number,
            } => self.finish_empty_pending(old_log_number, table_file_number, &limits),
            FlushWork::BuildTable {
                entries,
                old_log_number,
                table_file_number,
            } => self.build_and_install_table(
                &filesystem,
                &limits,
                entries,
                old_log_number,
                table_file_number,
            ),
        }
    }

    /// Возвращает неразрушающий file inventory текущего VersionSet.
    pub fn file_inventory(&self) -> Result<FileInventory> {
        self.ensure_open()?;
        let (manifest, version) = {
            let state = self.lock_state()?;
            (state.manifest.clone(), Arc::clone(&state.version))
        };
        inventory_for(version.as_ref(), &manifest)
    }

    /// Выполняет quick или deep verification live state.
    pub fn verify(&self, verify_options: VerifyOptions) -> Result<VerifyReport> {
        self.ensure_open()?;
        let limits = self.options.limits()?;
        let (manifest, version) = {
            let state = self.lock_state()?;
            (state.manifest.clone(), Arc::clone(&state.version))
        };
        let inventory = inventory_for(version.as_ref(), &manifest)?;
        verify_version(
            version.as_ref(),
            self.identity,
            &limits,
            self.options.max_compaction_bytes,
            inventory,
            self.snapshots.oldest_active_sequence()?,
            verify_options,
        )
    }

    /// Синхронизирует WAL и MANIFEST и освобождает process lock.
    pub fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let mut state = self.lock_state_allow_closed()?;
        if state.flush_in_progress {
            self.closed.store(false, Ordering::SeqCst);
            return Err(Error::Busy {
                reason: "нельзя закрыть базу во время flush".to_owned(),
            });
        }
        if let Some(writer) = state.wal.take() {
            writer.sync()?;
        }
        if let Some(writer) = state.manifest_writer.take() {
            writer.sync()?;
        }
        state.lock.take();
        Ok(())
    }

    /// Возвращает persisted UUID базы.
    #[must_use]
    pub const fn identity(&self) -> DatabaseId {
        self.identity
    }

    /// Возвращает путь открытого каталога.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.directory
    }

    fn prepare_flush(&self, filesystem: &NativeFileSystem, limits: &Limits) -> Result<FlushWork> {
        let mut state = self.lock_state()?;
        ensure_writer_healthy(&state)?;
        if state.flush_in_progress {
            return Err(Error::Busy {
                reason: "другой flush уже выполняется".to_owned(),
            });
        }

        if state.pending_flush.is_none() {
            if state.mutable.is_empty() {
                return Ok(FlushWork::Noop);
            }
            state
                .wal
                .as_ref()
                .ok_or(Error::InternalInvariant {
                    invariant: "I-WAL-WRITER-PRESENT",
                    reason: "flush не содержит current WAL writer".to_owned(),
                })?
                .sync()?;
            let table_file_number = state.manifest.next_file_number;
            let new_log_number = table_file_number.checked_add(1).ok_or(
                Error::ResourceLimit {
                    resource: "file_number",
                    limit: u64::MAX,
                    observed: u64::MAX,
                },
            )?;
            let next_file_number = new_log_number.checked_add(1).ok_or(
                Error::ResourceLimit {
                    resource: "file_number",
                    limit: u64::MAX,
                    observed: u64::MAX,
                },
            )?;
            let old_log_number = state.manifest.log_number;
            let new_wal = create_wal(
                filesystem,
                &self.directory,
                self.identity,
                new_log_number,
            )?;
            let rotation = VersionEdit {
                log_number: Some(new_log_number),
                previous_log_number: Some(Some(old_log_number)),
                next_file_number: Some(next_file_number),
                last_sequence: Some(state.last_sequence),
                ..VersionEdit::default()
            };
            append_manifest_edit(&mut state, &rotation, limits)?;
            let frozen = std::mem::take(&mut state.mutable);
            state.pending_flush = Some(PendingFlush {
                memtable: frozen,
                old_log_number,
                table_file_number,
            });
            state.wal = Some(new_wal);
        }

        state.flush_in_progress = true;
        let pending = state
            .pending_flush
            .as_ref()
            .ok_or(Error::InternalInvariant {
                invariant: "I-PENDING-FLUSH-PRESENT",
                reason: "flush не содержит immutable memtable".to_owned(),
            })?;
        if pending.memtable.is_empty() {
            return Ok(FlushWork::EmptyPending {
                old_log_number: pending.old_log_number,
                table_file_number: pending.table_file_number,
            });
        }
        Ok(FlushWork::BuildTable {
            entries: pending.memtable.table_entries(),
            old_log_number: pending.old_log_number,
            table_file_number: pending.table_file_number,
        })
    }

    fn finish_empty_pending(
        &self,
        old_log_number: u64,
        table_file_number: u64,
        limits: &Limits,
    ) -> Result<()> {
        let result = (|| -> Result<()> {
            let mut state = self.lock_state()?;
            validate_pending(&state, old_log_number, table_file_number)?;
            let edit = VersionEdit {
                previous_log_number: Some(None),
                last_sequence: Some(state.last_sequence),
                ..VersionEdit::default()
            };
            append_manifest_edit(&mut state, &edit, limits)?;
            state.pending_flush = None;
            state.flush_in_progress = false;
            Ok(())
        })();
        if let Err(error) = result {
            self.clear_flush_flag()?;
            return Err(error);
        }
        self.remove_obsolete_wal(old_log_number)
    }

    fn build_and_install_table(
        &self,
        filesystem: &NativeFileSystem,
        limits: &Limits,
        entries: Vec<forgedb_sstable::TableEntry>,
        old_log_number: u64,
        table_file_number: u64,
    ) -> Result<()> {
        if let Err(error) = remove_if_exists(
            filesystem,
            &self.directory.join(sst_file_name(table_file_number)),
            &self.directory,
        ) {
            self.clear_flush_flag()?;
            return Err(error);
        }

        let build_result = (|| -> Result<_> {
            let temporary_directory = self.directory.join(TEMP_DIRECTORY_NAME);
            let metadata = build_table(
                filesystem,
                TableBuildRequest::new(
                    &self.directory,
                    &temporary_directory,
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
                filesystem,
                &self.directory,
                self.identity,
                &metadata,
                limits,
                self.options.max_compaction_bytes,
            )?;
            Ok((metadata, reader))
        })();
        let (metadata, reader) = match build_result {
            Ok(result) => result,
            Err(error) => {
                self.clear_flush_flag()?;
                return Err(error);
            }
        };

        let installation_result = (|| -> Result<()> {
            let mut state = self.lock_state()?;
            validate_pending(&state, old_log_number, table_file_number)?;
            let next_version = state
                .version
                .with_l0(metadata.clone(), reader, limits)?;
            let installation = VersionEdit {
                previous_log_number: Some(None),
                last_sequence: Some(state.last_sequence),
                added_files: vec![metadata],
                ..VersionEdit::default()
            };
            append_manifest_edit(&mut state, &installation, limits)?;
            state.version = Arc::new(next_version);
            state.pending_flush = None;
            state.flush_in_progress = false;
            Ok(())
        })();
        if let Err(error) = installation_result {
            self.clear_flush_flag()?;
            return Err(error);
        }
        self.remove_obsolete_wal(old_log_number)
    }

    fn remove_obsolete_wal(&self, old_log_number: u64) -> Result<()> {
        let filesystem = NativeFileSystem;
        remove_if_exists(
            &filesystem,
            &self.directory.join(wal_file_name(old_log_number)),
            &self.directory,
        )
    }

    fn resolve_read_sequence(
        &self,
        state: &DatabaseState,
        read_options: &ReadOptions,
    ) -> Result<Option<SequenceNumber>> {
        let Some(snapshot) = &read_options.snapshot else {
            return Ok(state.last_sequence);
        };
        if !snapshot.belongs_to(self.identity, self.handle_id) {
            return Err(Error::invalid_argument(
                "snapshot",
                "snapshot создан другой базой или другим open handle",
            ));
        }
        Ok(snapshot.sequence())
    }

    fn clear_flush_flag(&self) -> Result<()> {
        let mut state = self.lock_state()?;
        state.flush_in_progress = false;
        Ok(())
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, DatabaseState>> {
        self.ensure_open()?;
        self.lock_state_allow_closed()
    }

    fn lock_state_allow_closed(&self) -> Result<MutexGuard<'_, DatabaseState>> {
        self.state.lock().map_err(|_| Error::InternalInvariant {
            invariant: "I-DATABASE-MUTEX",
            reason: "mutex состояния базы poisoned".to_owned(),
        })
    }
}

fn recover_previous_wal(
    filesystem: &NativeFileSystem,
    directory: &Path,
    identity: DatabaseId,
    manifest: &ManifestState,
    limits: &Limits,
    memory_limit: usize,
    recovered_last: &mut Option<SequenceNumber>,
) -> Result<Option<PendingFlush>> {
    let Some(previous_log_number) = manifest.previous_log_number else {
        return Ok(None);
    };
    let table_file_number = manifest
        .log_number
        .checked_sub(1)
        .filter(|number| *number != 0)
        .ok_or_else(|| {
            Error::corruption(
                "flush recovery",
                "current WAL не позволяет восстановить table file number",
            )
        })?;
    if table_file_number <= previous_log_number {
        return Err(Error::corruption(
            "flush recovery",
            "table file number не новее previous WAL",
        ));
    }

    let mut immutable = MvccMemTable::default();
    let previous_path = directory.join(wal_file_name(previous_log_number));
    let summary = replay_wal_numbered(
        filesystem,
        &previous_path,
        identity,
        previous_log_number,
        limits,
        |batch| apply_replayed_batch(&mut immutable, batch, limits, memory_limit),
    )?;
    *recovered_last = maximum_sequence(*recovered_last, summary.last_sequence);
    Ok(Some(PendingFlush {
        memtable: immutable,
        old_log_number: previous_log_number,
        table_file_number,
    }))
}

fn append_manifest_edit(
    state: &mut DatabaseState,
    edit: &VersionEdit,
    limits: &Limits,
) -> Result<()> {
    let result = {
        let DatabaseState {
            manifest_writer,
            manifest,
            ..
        } = state;
        manifest_writer
            .as_mut()
            .ok_or(Error::InternalInvariant {
                invariant: "I-MANIFEST-WRITER-PRESENT",
                reason: "read-write база не содержит MANIFEST writer".to_owned(),
            })?
            .append_edit(manifest, edit, limits)
    };
    if let Err(error) = &result {
        state.fatal_write_error = Some(error.to_string());
    }
    result
}

fn validate_pending(
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
    if pending.old_log_number != old_log_number
        || pending.table_file_number != table_file_number
    {
        return Err(Error::InternalInvariant {
            invariant: "I-FLUSH-GENERATION-STABLE",
            reason: "поколение flush изменилось во время построения SSTable".to_owned(),
        });
    }
    Ok(())
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

fn remove_if_exists(
    filesystem: &NativeFileSystem,
    path: &Path,
    directory: &Path,
) -> Result<()> {
    match filesystem.remove_file(path) {
        Ok(()) => filesystem.sync_directory(directory),
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn apply_replayed_batch(
    memtable: &mut MvccMemTable,
    wal_batch: WalBatch,
    limits: &Limits,
    memory_limit: usize,
) -> Result<()> {
    let prepared = memtable.prepare(
        wal_batch.sequence_start,
        &wal_batch.batch,
        limits,
        memory_limit,
    )?;
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

fn inventory_for(version: &VersionSet, manifest: &ManifestState) -> Result<FileInventory> {
    let numbers = version
        .files_in_read_order()?
        .into_iter()
        .map(|table| table.metadata().file_number)
        .collect::<Vec<_>>();
    FileInventory::build(version.directory(), manifest, numbers)
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
        let path = std::env::temp_dir().join(format!(
            "forgedb-v030-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn snapshot_survives_overwrite_and_flush() -> Result<()> {
        let directory = temporary_directory("snapshot-flush")?;
        let db = Database::open(&directory, Options::default())?;
        db.put(b"key", b"old", WriteOptions::sync())?;
        let snapshot = db.snapshot()?;
        db.put(b"key", b"new", WriteOptions::sync())?;
        db.flush()?;
        assert_eq!(db.get(b"key")?, Some(b"new".to_vec()));
        assert_eq!(
            db.get_with_options(b"key", ReadOptions::from_snapshot(&snapshot))?,
            Some(b"old".to_vec())
        );
        db.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn scan_matches_reference_model_across_flushes() -> Result<()> {
        let directory = temporary_directory("scan-model")?;
        let db = Database::open(&directory, Options::default())?;
        let mut model = BTreeMap::<Vec<u8>, Option<Vec<u8>>>::new();
        for index in 0..120_u64 {
            let key = format!("key-{index:03}").into_bytes();
            if index % 7 == 0 {
                db.delete(&key, WriteOptions::sync())?;
                model.insert(key, None);
            } else {
                let value = format!("value-{index}").into_bytes();
                db.put(&key, &value, WriteOptions::sync())?;
                model.insert(key, Some(value));
            }
            if index % 31 == 30 {
                db.flush()?;
            }
        }
        let actual = db
            .scan(.., ReadOptions::default())?
            .collect::<Result<Vec<_>>>()?;
        let expected = model
            .into_iter()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        db.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn foreign_snapshot_is_rejected() -> Result<()> {
        let first_directory = temporary_directory("foreign-first")?;
        let second_directory = temporary_directory("foreign-second")?;
        let first = Database::open(&first_directory, Options::default())?;
        let second = Database::open(&second_directory, Options::default())?;
        let snapshot = first.snapshot()?;
        assert!(second
            .get_with_options(b"key", ReadOptions::from_snapshot(&snapshot))
            .is_err());
        first.close()?;
        second.close()?;
        fs::remove_dir_all(&first_directory).map_err(|source| {
            Error::io("remove_test_directory", Some(first_directory), source)
        })?;
        fs::remove_dir_all(&second_directory).map_err(|source| {
            Error::io("remove_test_directory", Some(second_directory), source)
        })?;
        Ok(())
    }

    #[test]
    fn deep_verify_reads_installed_table() -> Result<()> {
        let directory = temporary_directory("verify")?;
        let db = Database::open(&directory, Options::default())?;
        db.put(b"alpha", b"one", WriteOptions::sync())?;
        db.flush()?;
        let report = db.verify(VerifyOptions::deep())?;
        assert_eq!(report.live_sstables, 1);
        assert_eq!(report.entries_checked, 1);
        db.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
