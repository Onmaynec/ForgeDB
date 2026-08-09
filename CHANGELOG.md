# История изменений

В этом файле перечисляются заметные изменения ForgeDB.

До v1.0 публичный API может меняться между minor-версиями. Persisted-форматы всё равно изменяются только через явную новую версию, ADR, compatibility tests и документированную migration policy.

## [0.5.0] — 2026-08-09

### Добавлено

- Public `Database::begin_transaction()`, `Transaction` и `TransactionState`.
- Optimistic snapshot isolation: snapshot фиксируется при BEGIN и удерживается до terminal state.
- Own-write visibility: transaction читает последнюю buffered mutation раньше snapshot database state.
- Buffered `put`/`delete` и explicit rollback без WAL mutation.
- Write-write conflict detection по committed sequence numbers во всех live sources: mutable memtable, pending immutable memtable, L0 и L1+ SSTables.
- Единый public write gate, сериализующий conflict validation и последующий WAL append относительно обычных public writes.
- Atomic transaction commit одним существующим `WriteBatch` через WAL V1 path.
- Bounded transaction read set.
- CLI `forgedb transaction` / `forgedb tx` с `--put`, `--delete`, `--get`, `--rollback`, `--sync` и `--no-sync`.
- Stable CLI exit code `9` для transaction conflict.
- Deterministic transaction state-machine/reference-model test.
- CLI integration tests для commit, rollback и запрета `--disable-wal`.
- Документы `docs/TRANSACTIONS.md`, ADR 0011 и `docs/releases/V0.5.0.md`.

### Изменено

- Workspace version повышена до `0.5.0`.
- `StreamingDatabase` использует shared public write gate между clone handles.
- MVCC memtable предоставляет newest sequence lookup для conflict validation.
- `VersionSet` предоставляет correctness-first latest-sequence lookup по live SSTables с checksum validation.
- Compatibility documentation обновлена для v0.5.0.
- MANIFEST V1 compatibility fixture приведён к strict stable Clippy (`&Path` вместо `&PathBuf`).

### Гарантии transactions

- Transaction read snapshot не видит committed sequence выше sequence на BEGIN.
- Own buffered writes видимы самой transaction до commit и не видимы другим readers.
- Если written key изменился после BEGIN, commit отклоняется целиком, даже если value bytes затем вернулись к прежнему значению.
- После successful validation весь write set передаётся одним atomic WAL batch; partial transaction recovery не вводится.
- `DisableWal` для transaction commit запрещён.
- Active transaction участвует в existing SnapshotRegistry и тем самым в compaction retention oldest active snapshot.

### Persisted-форматы

v0.5.0 не меняет persisted layout.

| Формат | Read support | Write support |
|---|---|---|
| IDENTITY | 1 | 1 |
| Internal Key | 1 | 1 |
| WAL | 1 | 1 |
| SSTable | 1 | 1 |
| MANIFEST | 1, 2 | 2 |
| CURRENT | 1 | 1 |

### Ограничения

- Isolation — snapshot isolation, не serializable.
- Обязательна write-write conflict detection; full read-write validation пока отсутствует.
- Write skew, predicate/range locks и phantom protection не предотвращаются.
- Nested transactions, savepoints и automatic retry отсутствуют.
- Full FQL parser/shell остаётся milestone v0.8.0.
- SSTable conflict lookup correctness-first и может быть оптимизирован в v0.6 только после benchmark.

## [0.4.0] — 2026-08-07

### Добавлено

- Physical leveled compaction L0 → L1 и Ln → Ln+1.
- L0 overlap-closure picker и size-based picker для L1+.
- MVCC retention на основе `oldest_active_snapshot_sequence`.
- Conservative bottommost tombstone dropping.
- Output splitting без разрыва versions одного user key.
- Durable reservation output file numbers перед созданием compaction SSTables.
- Atomic MANIFEST installation: delete inputs + add outputs одним VersionEdit.
- Persisted L1+ metadata.
- MANIFEST V2 с `ADD_LEVEL_FILE` и `DELETE_LEVEL_FILE`.
- Чтение MANIFEST V1 и V2.
- Crash-safe V1 → V2 rotation при writable open; read-only V1 open не мигрирует данные.
- L0 slowdown/stop backpressure перед public writes.
- Automatic synchronous compaction после `flush()`.
- Public `Database::compact_range`.
- CLI `compact <database> [start] [end-exclusive]`.
- MANIFEST V1/V2 compatibility tests.
- Reopen regression test после L0 → L1 compaction.
- Unit tests snapshot boundary retention, bottommost tombstone и output splitting.
- Документы `docs/COMPACTION.md`, `docs/formats/MANIFEST_V2.md`, ADR 0010 и `docs/releases/V0.4.0.md`.

### Изменено

- Workspace version повышена до `0.4.0`.
- `VersionSet::open` восстанавливает physical L1+ из MANIFEST.
- `ManifestState` валидирует unique file numbers между всеми уровнями и non-overlap L1+.
- New databases записывают MANIFEST V2; writer v0.4 больше не создаёт новый V1 MANIFEST.
- WAL и SSTable persisted formats остаются V1.
- `flush()` после installation проверяет compaction pressure и может синхронно выполнить compaction.

### Crash consistency

- File numbers compaction outputs резервируются synced MANIFEST edit до создания файлов и не переиспользуются после crash.
- Output SSTables полностью проходят temp/write/sync/rename/directory-sync до installation edit.
- До synced installation authoritative остаются input files.
- После synced installation authoritative являются output files в target level.
- Unreferenced SST после crash не включается normal recovery автоматически.

### Persisted-форматы

| Формат | Текущая версия | Read support | Write support |
|---|---:|---|---|
| IDENTITY | 1 | 1 | 1 |
| Internal Key | 1 | 1 | 1 |
| WAL | 1 | 1 | 1 |
| SSTable | 1 | 1 | 1 |
| MANIFEST | 2 | 1, 2 | 2 |
| CURRENT | 1 | 1 | 1 |

### Ограничения

- Compaction остаётся synchronous; background worker pool ещё не реализован.
- Compaction job bounded `max_compaction_bytes`, но materialize-ит выбранные entries в пределах этого budget.
- Physical input SST после compaction пока не удаляются автоматически: reference-aware Windows-safe obsolete reaper ещё не реализован.
- Reverse iterator и transactions отсутствуют.
- Compaction metrics и range tombstones отсутствуют.

## [0.3.0] — 2026-08-05

### Добавлено

- MVCC memtable, сохраняющая версии internal keys.
- Cheap snapshots с registry активных sequence и проверкой принадлежности open handle.
- `ReadOptions` и snapshot-aware point reads.
- Immutable `VersionSet` с L0 newest-first lookup.
- Validated модель L1+ с sorted/non-overlap invariant.
- Forward merge iterator на binary heap.
- Block-at-a-time SSTable cursors и bounded resident memory.
- Public range scan с bounds и limit.
- File inventory с live/obsolete/orphan/temp/backup/user/unknown категориями.
- Неразрушающий `RecoveryPlan::inspect`.
- Quick/deep verify reports.
- CLI `scan`, `inventory`, `verify`, `verify --deep` и `inspect recovery-plan`.
- Model, snapshot, cursor, level-invariant, recovery-plan и deep-verify tests.
- Русские документы Full LSM read path, snapshots/MVCC и inventory/recovery/verify.

### Исправлено

- Read-write open больше не может пересоздать отсутствующий MANIFEST-owned current или previous WAL пустым файлом.
- Все MANIFEST-owned WAL проверяются до replay и append-open; отсутствие классифицируется как corruption.
- Range scan больше не materialize-ит SSTable целиком и удерживает только текущий data block каждого cursor.
- Чистая история v0.3 пересобрана поверх выпущенного `v0.2.0` без старых stacked commits.

### Изменено

- Workspace version повышена до `0.3.0`.
- Read path объединяет mutable, immutable и все файлы текущего VersionSet.
- Persisted-форматы остаются V1; новая версия формата не добавлена.
- L1+ пока представлены validated in-memory model и будут физически заполняться compaction milestone v0.4.0.

### Проверка

- Feature gate на head `620a90e9bd697c63efe92b4fee6c65f8636bf571` прошёл GitHub Actions run `31009363168`.
- Ubuntu stable и Windows stable прошли canonical rustfmt, strict Clippy `-D warnings`, workspace tests, doctests и release build.
- Ubuntu Rust 1.89 прошёл strict Clippy и workspace tests.

### Persisted-форматы

| Формат | Текущая версия | Чтение | Запись |
|---|---:|---:|---:|
| IDENTITY | 1 | 1 | 1 |
| Internal Key | 1 | 1 | 1 |
| WAL | 1 | 1 | 1 |
| SSTable | 1 | 1 | 1 |
| MANIFEST | 1 | 1 | 1 |
| CURRENT | 1 | 1 | 1 |

### Ограничения

- Физические L1+ files и leveled compaction ещё отсутствуют.
- Reverse iterator и transactions ещё отсутствуют.
- Flush синхронный; background compaction и write stalls появятся позже.
- Произвольные orphan/unknown-файлы не удаляются автоматически.

## [0.2.0] — 2026-08-05

### Добавлено

- Crate `forgedb-sstable` с версионируемым форматом SSTable V1.
- Фиксированные header/footer, checksummed data/index/filter/properties blocks.
- Prefix compression с restart points и строгим bounded decoder.
- Versioned Bloom filter без false negatives для добавленных ключей.
- Полная проверка database UUID, file number, sequence range, offsets, lengths и CRC32C.
- Crate `forgedb-manifest` с MANIFEST V1, VersionEdit и crash-safe CURRENT.
- Единая монотонная нумерация WAL, SSTable и MANIFEST metadata.
- Mutable/immutable memtable lifecycle для синхронного flush.
- Двухфазный WAL rotation и SSTable installation protocol.
- `Database::flush()` и point reads в порядке mutable → immutable → L0 от новых к старым.
- Recovery current и previous WAL в разных memtable generations.
- WAL-first bootstrap: initial WAL синхронизируется до публикации MANIFEST/CURRENT.
- Strict idempotent creation для header-only orphan WAL после crash до rotation edit.
- Fail-safe отказ от повторного использования WAL с complete frame или partial tail.
- Deterministic cleanup только reserved bootstrap artifacts при отсутствии CURRENT.
- Безопасное игнорирование unreferenced SSTable: orphan не включается в live state автоматически.
- Удаление obsolete WAL только после успешного sync MANIFEST installation edit.
- `flush_in_progress` и `Busy` для concurrent flush/close.
- Fatal writer state после MANIFEST append/sync error.
- `TableBuildRequest` для типизированного SSTable builder API без длинного позиционного списка аргументов.
- Integration tests reopen-after-flush, deterministic SST checksum corruption, orphan SST, missing live WAL, WAL bootstrap/rotation retry и deterministic model.
- Crash recovery matrix `docs/CRASH_RECOVERY_MATRIX_V0.2.md`.
- Release notes `docs/releases/V0.2.0.md`.
- Русские нормативные документы SSTable V1, MANIFEST V1 и CURRENT V1.

### Исправлено

- Previous и current WAL больше не смешиваются в одну mutable memtable после crash между rotation и installation.
- Повторный crash не может исключить previous WAL из MANIFEST до установки соответствующего SSTable.
- Отсутствующий MANIFEST-owned WAL больше не пересоздаётся пустым и блокирует open как corruption.
- Header-only WAL, оставшийся до публикации CURRENT или rotation edit, может быть безопасно повторно использован.
- Non-empty orphan WAL сохраняется и блокирует flush вместо удаления или truncate.
- Windows bootstrap retry не зависит от replacement semantics `std::fs::rename`: reserved destination очищается до установки нового MANIFEST.
- `Database::close()` публикует closed state только после успешного WAL/MANIFEST sync.
- Старый небезопасный `database.rs` удалён; активна generation-safe state machine.
- Рассинхронизированный `Cargo.lock` обновлён до полного workspace `0.2.0`.
- Flaky corruption-тесты теперь гарантированно изменяют сохранённый CRC через XOR.
- Удалены локальные `clone_on_copy` и `too_many_arguments` lint-исключения из `forgedb-sstable`.

### Изменено

- Workspace version повышена до `0.2.0`.
- WAL получил canonical numbered generations.
- Public `create_wal` идемпотентен только для строго проверенного header-only generation.
- Public `create_manifest` гарантирует WAL-before-CURRENT bootstrap и запрещает reinitialize при существующем CURRENT.
- Memtable различает `Missing`, `Value` и `Tombstone`.
- Открытие базы проверяет все SSTables и WAL, на которые ссылается MANIFEST.
- CI использует `actions/checkout@v6`, зафиксированный `Cargo.lock` и `--locked` для Clippy/tests/release build.
- Все crates наследуют единый strict workspace lint policy.
- Release gate подтверждён на Ubuntu stable, Windows stable и Rust 1.89.

### Persisted-форматы

| Формат | Текущая версия | Чтение | Запись |
|---|---:|---:|---:|
| IDENTITY | 1 | 1 | 1 |
| Internal Key | 1 | 1 | 1 |
| WAL | 1 | 1 | 1 |
| SSTable | 1 | 1 | 1 |
| MANIFEST | 1 | 1 | 1 |
| CURRENT | 1 | 1 | 1 |

### Ограничения

- Только L0; leveled compaction ещё отсутствует.
- Range scan, snapshots и transactions ещё отсутствуют.
- Flush синхронный; background worker и write stalls появятся позже.
- SSTable reader версии 0.2 bounded-загружает файл в память.
- Произвольные orphan/unknown-файлы не удаляются автоматически и пока не имеют отдельного CLI inventory report.

## [0.1.0] — 2026-08-03

### Добавлено

- Новый crate `forgedb-wal` с форматом WAL V1.
- Фиксированный versioned-заголовок WAL и один атомарный кадр на `WriteBatch`.
- CRC32C Castagnoli для заголовка файла и каждого полного кадра.
- Bounded decoder с проверкой длин, operation count, varint, reserved fields и sequence range до allocation.
- Строгий replay без resynchronization после corruption.
- Безопасная обработка и синхронизированное усечение только последнего физически неполного кадра.
- Mutable memtable на `BTreeMap` с preflight memory accounting.
- `Database::open`, `put`, `get`, `delete`, `write` и `close`.
- Режимы `Sync`, `NoSync` и `DisableWal`.
- Атомарное применение batch после требуемой durability boundary.
- Read-only открытие с replay и запретом записи.
- CLI-команды `put`, `get` и `delete`.
- Process-abort test для подтверждённой `Sync`-записи.
- Deterministic model test, corruption tests и тест sequence-gap после `DisableWal`.
- Русская нормативная документация WAL, write/read path, recovery, durability и compatibility.

### Изменено

- Workspace version повышена до `0.1.0`.
- `Database::close` синхронизирует WAL и освобождает системную блокировку.
- Файловая абстракция получила синхронизированный `truncate_file`.
- Replay разрешает возрастающие sequence gaps, но запрещает пересечения и откат.

### Гарантии

- Успешный `Sync` выполняет WAL `sync_all` до видимости batch в memtable.
- Полный batch либо целиком replayed, либо не применяется.
- Checksum mismatch полного кадра блокирует открытие.
- Неизвестные будущие версии persisted-форматов отклоняются.

### Ограничения

- Нет SSTable, MANIFEST, flush, compaction, snapshots и transactions.
- Используется один фиксированный WAL без rotation.
- Данные находятся только в mutable memtable и восстанавливаются из WAL при каждом открытии.

## [0.0.1] — 2026-08-03

### Добавлено

- Cargo workspace с crates `forgedb`, `forgedb-core` и `forgedb-fs`.
- Rust Edition 2024 и минимальная поддерживаемая версия Rust 1.89.
- Единая модель ошибок и проверяемые `Options`.
- Примитивы `DatabaseId`, `SequenceNumber`, `ColumnFamilyId` и `ValueType`.
- Логический comparator internal keys и bounded codec Internal Key V1.
- `WriteBatch` с атомарным изменением, лимитами и checked size accounting.
- Файловая абстракция и native-реализация.
- Системная эксклюзивная блокировка базы.
- Версионируемый и синхронизируемый файл `IDENTITY`.
- Unit tests, deterministic property tests, Linux/Windows CI, документация и ADR.

### Не входило в версию

- WAL, memtable, SSTable, MANIFEST, read/write API и гарантии пользовательских данных отсутствовали.
