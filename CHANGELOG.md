# История изменений

В этом файле перечисляются заметные изменения ForgeDB.

До v1.0 публичный API может меняться между minor-версиями. Persisted-форматы всё равно изменяются только через явную новую версию, ADR, compatibility tests и документированную migration policy.

## [0.4.0] — Unreleased

### Добавлено

- Новый внутренний crate `forgedb-lsm` с deterministic compaction scoring и picker-ами.
- L0→L1 overlap closure и Ln→Ln+1 target selection.
- Отдельный manual range picker, не зависящий от background pressure в других диапазонах.
- Bounded snapshot-aware heap merge с duplicate-sequence и source-order validation.
- Snapshot-floor MVCC retention и bottommost tombstone rules.
- Streaming output protocol `BeginFile → Entry* → FinishFile` без разделения versions одного logical key.
- MANIFEST V2 с exact level add/delete records, level count и compaction pointers.
- Fixed MANIFEST V1 fixture, exact V1/V2 dispatch и future-version rejection.
- Crash-safe V1→V2 migration под exclusive database lock.
- Production `Database` path на MANIFEST V2 и physical L1+ SSTables.
- Public synchronous `Database::compact_range` с half-open range semantics.
- Bounded exhaustive обработка всех исходных source SST диапазона с progress proof и hard limit 10 000 steps.
- Durable compaction installation: file-number reservation, SST sync/rename, final atomic MANIFEST edit и immutable VersionSet swap.
- Reference-aware obsolete SST queue на `Weak<VersionSet>`; streaming cursors удерживают `Arc` lease.
- Bounded L0 write slowdown и stop threshold с `Error::Busy`.
- Child-process process-crash matrix для installation boundaries до/после MANIFEST sync и перед obsolete cleanup.
- Нормативные документы MANIFEST V2, compaction architecture и compatibility/migration policy.

### Исправлено

- `ManifestStateV2::initial` теперь вычисляет `next_file_number` как `max(manifest_number, log_number) + 1` и не допускает reuse active MANIFEST number.
- `file_inventory()` больше не выполняет filesystem cleanup и остаётся строго read-only.
- Compaction replacement отклоняет missing/duplicate inputs, output metadata mismatch, live file-number reuse и L1+ overlap.
- Crash до final compaction edit не делает orphan output live.
- Crash после final MANIFEST sync восстанавливает новый VersionSet даже до in-memory swap.
- Active streaming iterator откладывает physical deletion input SST до освобождения старой Version.
- Write slowdown выполняется вне database state lock и повторно проверяет stop threshold после delay.

### Изменено

- Workspace version повышена до `0.4.0`.
- Active production MANIFEST writer изменён с V1 на V2.
- Read-only V1 open использует in-memory bridge без mutation; read-write V1 open выполняет durable migration.
- Flush installation записывает level-aware V2 edits.
- Manual compaction newly created outputs не продвигает через все levels в том же вызове; следующий вызов может продолжить.
- Obsolete cleanup запускается только mutating paths: write, flush, compaction и close.

### Проверка

Последние подтверждённые clean read-only gates:

- production MANIFEST V2 runtime: run `31022988314`;
- physical compaction installation: run `31024564376`;
- obsolete lifetime: run `31025353818`;
- write stalls: run `31025868894`;
- read-only inventory: run `31026106815`;
- requested-range picker: run `31026923116`;
- bounded exhaustive manual source processing: run `31028055579`;
- process-crash installation matrix: head `d2f7902650506bf287a68575f5d2fe1e874216e7`, run `31029335050`.

Каждый gate прошёл Ubuntu stable, Windows stable и Ubuntu Rust 1.89. Stable jobs выполняют canonical rustfmt, strict Clippy `-D warnings`, workspace tests, doctests и release build.

### Persisted-форматы

| Формат | Active writer | Read support |
|---|---:|---:|
| IDENTITY | 1 | 1 |
| Internal Key | 1 | 1 |
| WAL | 1 | 1 |
| SSTable | 1 | 1 |
| MANIFEST | 2 | 1, 2 |
| CURRENT | 1 | 1 |

WAL V1, SSTable V1 и Internal Key V1 не изменены. MANIFEST V1 остаётся legacy read-compatible и мигрируется только при read-write open под lock.

### Ограничения

- Compaction синхронна и удерживает database state lock во время merge/build/install.
- Background compaction scheduler и compaction/stall metrics ещё отсутствуют.
- Slowdown delay не interruptible в пределах hard cap 50 ms.
- Obsolete cleanup intent не persisted; process crash может оставить безопасный orphan SST.
- Process-crash matrix не доказывает OS-crash или power-loss guarantees.
- Disk-full и injected sync/rename failure matrix относятся к hardening milestone.

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
