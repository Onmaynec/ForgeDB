# Архитектура ForgeDB v0.8.2

## Назначение

ForgeDB — встраиваемый key-value storage engine с LSM-структурой и собственными persisted-форматами. Порядок приоритетов: корректность записи и восстановления, проверяемые ограничения ресурсов, совместимость файлов и только затем производительность.

## Column Families v0.7.0

`ColumnFamilies` управляет одним legacy default database root и зарегистрированными named child database roots. Каждая family — независимая LSM/WAL топология с собственными snapshots, read/write gates, MVCC sequence numbers и process lock. Это **не** multiple column families внутри одного физического WAL/SST/MANIFEST.

Слой `column_families.rs` хранит реестр `CF_REGISTRY V1` (`FGCFREG1`, CRC32C, next_id, ID↔name) в `.forgedb-column-families/REGISTRY`. Создание: инициализация child DB → close/sync → полная запись временного registry → sync → atomic rename → sync directories → publication in-memory. Drop удаляет запись из registry только после close child, каталог оставляет orphan. Snapshot и transaction относятся к одному child DB; cross-family atomic batch не поддерживается.

Default API `Database::open` по-прежнему читает/пишет только прежние файлы корневой базы. Миграции WAL V1/SSTable V1/MANIFEST V2 нет. Новый интерфейс и CF registry документированы в [ADR 0015](adr/0015-column-family-isolation.md) и [CF_REGISTRY V1](formats/CF_REGISTRY_V1.md).

## FQL V1 / CLI query layer (v0.8.0)

Модуль `crates/forgedb-cli/src/fql.rs` содержит отдельный bounded lexer/parser и FQL session. Любая операция сначала полностью преобразуется в AST, затем исполняется через `ColumnFamilies`. One-shot `fql`, shell из stdin и file script используют один executor. FQL не знает внутреннего layout SSTable/MANIFEST и не вводит новые persisted-файлы.

`BEGIN/COMMIT` переиспользует optimistic `Transaction` текущей named family; `USE/CREATE/DROP` и `SCAN` до COMMIT запрещены. В интерактивной сессии неправильный statement не прерывает работу; в скрипте fail-fast с номером строки. Незавершённый BEGIN откатывается при выходе; уже committed прежние строки скрипта не откатываются.

Парсер ограничивает statement 64 KiB, число токенов 512, `SCAN` — 100 строк по умолчанию, максимум 10 000. Binary literal поддерживает quoted escapes и `X'HEX'`, вывод GET/SCAN обратимо экранируется. Формат FQL описан в [FQL.md](FQL.md), проектные решения — в [ADR 0016](adr/0016-fql-cli.md).

## Native SST handle pool (v0.8.1)

`TableHandlePool` находится в `forgedb-sstable`. В `VersionSet::open` каждый SST metadata/header/index/filter/properties проверяется синхронно, после чего initial FD закрывается, а `TableReader` получает общий `Arc<TableHandlePool>`. Shared pool переживает публикацию новых поколений `VersionSet` и используется добавленными через flush/compaction readers.

После cache miss на data block общий pool mutex обеспечивает atomic LRU decision и exclusive seek/read. При reopen проверяются физический размер файла, DatabaseId/SST number, header checksum и sequence range; data block CRC32C не изменяется. Дескриптор выселяется до открытия следующего, поэтому pool residency не превышает `table_cache_capacity`. Mutex сериализует point SST I/O и может влиять на throughput.

Reference-aware deletion использует `Weak<TableReader>` для старых поколений, а не OS handles. Когда reader больше не жив, перед `remove_file` cached FD закрывается; это необходимая последовательность для Windows. Неизвестные orphan SST после restart не удаляются.

**Граница:** streaming cursors, compaction files, WAL/MANIFEST/LOCK и другие Column Families не входят в указанный per-DB point pool. См. [ADR 0017](adr/0017-bounded-sst-handle-cache.md) и Issue #31.

## Streaming cursor и compaction input FD pooling (v0.8.2)

Публичный `Database::scan` создаёт `SstEntryCursor::open_pooled`. При инициализации файл всё ещё полностью проходит проверку SST V1 header/footer/index/filter/properties и первого блока, но сразу после этого исходный FD закрывается. Cursor сохраняет index и один текущий data block, а следующие блоки читает через shared `TableHandlePool::with_file`.

Пул уже используется всеми поколениями `VersionSet`. Старые iterators удерживают `Arc<VersionSet>` (и `Arc<TableReader>`), а LRU eviction не освобождает право владения SST. Поэтому reference-aware cleanup по-прежнему защищает Windows-путь удаления retired SST.

Compaction inputs теперь выполняют `read_entries_pooled` на основе cursor, а не `fs::read` всего SST: физические handles читают блоки через общий pool. Однако compaction merge по-прежнему материализует записи; `max_compaction_bytes` контролирует объём. Read-only deep verify и output SST builder живут отдельно.

**Ресурсный контракт:** `table_cache_capacity` ограничивает resident File внутри одного SST read pool, но не кратковременные validation opens, SST writers, WAL/MANIFEST/LOCK или отдельные named child bases. См. [ADR 0018](adr/0018-streaming-sst-handle-leases.md). Issue #31 остаётся открытой.

## Состав workspace

| Crate | Ответственность |
|---|---|
| `forgedb-core` | ключи, sequence numbers, WriteBatch, Limits, ошибки |
| `forgedb-fs` | FileSystem, native I/O, LOCK, IDENTITY |
| `forgedb-wal` | WAL V1, numbered generations, CRC32C, replay |
| `forgedb-sstable` | SSTable V1, block index, Bloom filter, reader/builder |
| `forgedb-manifest` | CURRENT, MANIFEST V1/V2, VersionEdit, persisted file topology |
| `forgedb` | публичный Database API, memtables, snapshots, VersionSet, iterator, flush, compaction, inventory, verify |
| `forgedb-cli` | команды для разработчика поверх публичного API |

`forgedb-core` не зависит от подсистем хранения. WAL и SSTable не принимают решений об уровне LSM, а MANIFEST хранит authoritative topology. CLI не обходит публичный API.

## Runtime

Публичный `Database` экспортирует `StreamingDatabase`. Пока используется композиция исходников `database_v3_fixed.rs`, `database_streaming_wrapper.rs` и `compaction.rs` через `include!`. Это действующая реализация v0.6.2; объединение модулей — отдельный рефакторинг, не условие чтения persisted-форматов.

Внутреннее состояние хранит:

- mutable MVCC memtable и необязательную pending immutable memtable;
- current WAL writer и writer MANIFEST;
- `ManifestState` и текущий `Arc<VersionSet>`;
- следующий и последний опубликованный sequence;
- признак fatal write failure и `flush_in_progress`;
- нативную эксклюзивную блокировку каталога.

`SnapshotRegistry` отслеживает живые snapshot handles и oldest active sequence. Закрытие удерживает mutex состояния, синхронизирует оба writer-а и только после успеха публикует `closed`. Ошибка sync не удаляет writer handles и не освобождает LOCK; после получения mutex выполняется дополнительная проверка `closed`.

## Путь записи

Операции `put`, `delete` и `write` направляются в единый borrowed-batch write path. Под mutex выполняются validation, резервирование sequence range, preflight mutable memtable, WAL append и, для `Sync`, синхронизация. Только после успешной WAL boundary изменения публикуются в memtable. Если preflight отклоняет batch из-за `memtable_memory`, публичный wrapper делает один synchronous flush/retry; oversized batch будет отклонён повторно без частичной публикации.

`DisableWal` допускает потерю неподтверждённого на диске состояния при аварийном завершении. Полный контракт: [WRITE_PATH.md](WRITE_PATH.md) и [DURABILITY.md](DURABILITY.md).

## Путь чтения

Чтение учитывает текущий sequence или зарегистрированный snapshot. Источники: mutable memtable, pending immutable memtable, затем файлы актуального immutable `VersionSet` — L0 в порядке от новых к старым и non-overlapping L1+.

Публичный `scan` использует merge iterator и block-at-a-time SST cursors. Reader удерживает ссылку на выбранную версию LSM на время работы. Подробности: [READ_PATH.md](READ_PATH.md).

## Cache and indexed point lookup

`VersionSet` использует проверенный SSTable index/Bloom filter для snapshot-aware point reads и optimistic conflict validation. `BlockCache` держит bounded декодированные data blocks; физический `TableReader` в v0.6.2 хранит открытый random-access file handle и проверенные header/footer/index/filter/properties, не загружая весь SST. Data blocks читаются по offsets с проверкой CRC32C. Файл остаётся закреплённым на lifetime reader, поэтому process-local reaper ждёт всех readers (в том числе старых snapshot iterators). Публичный `cache_stats()` показывает hits/misses/evictions для декодированных blocks. Range scan использует отдельный block-at-a-time cursor.

Подробности: [CACHE_AND_PERFORMANCE.md](CACHE_AND_PERFORMANCE.md).

## Flush и compaction

Flush синхронизирует current WAL перед rotation, фиксирует новые номера файлов, создаёт immutable SSTable и публикует новую L0-топологию через synced MANIFEST edit. Recovery различает current и previous WAL поколения.

Leveled compaction выбирает L0→L1 или Ln→Ln+1, строит outputs и публикует их единым MANIFEST edit. До installation authoritative остаются inputs, после durable installation — outputs. Compaction синхронна; её входные SST могут быть удалены best-effort после synced MANIFEST edit только при отсутствии reader references. Ошибки unlink не меняют успешность durable compaction, повторная попытка доступна через явный cleanup. Правила: [COMPACTION.md](COMPACTION.md).

## Persisted-файлы

```text
LOCK                  эксклюзивная системная блокировка
IDENTITY              UUID базы, формат V1
CURRENT               имя актуального MANIFEST, формат V1
MANIFEST-*.log        authoritative edits/snapshot, reader V1/V2, writer V2
WAL-*.log             current/previous WAL V1
SST-*.sst             immutable SSTable V1
TEMP/                 временные файлы сборки
```

Reader отклоняет неизвестные future versions. Writable open старого MANIFEST V1 публикует новый V2 через crash-safe замену CURRENT, без изменения V1 in-place. Подробности: [COMPATIBILITY.md](COMPATIBILITY.md).

## Границы v0.6.2

- один public write gate и эксклюзивный process lock; shared read-only opens не поддерживаются;
- optimistic transactions предоставляют snapshot isolation, но не serializable isolation;
- compaction synchronous, её merge ограничен memory budget, но ещё не fully streaming;
- synchronous auto-flush/retry не заменяет фоновые flush workers;
- reverse iterator и range tombstones отсутствуют;
- process-local retired SST cleanup выполняется best-effort после compaction, restart-aware GC отсутствует;
- `table_cache_capacity` и параметры background jobs зарезервированы, полноценное lazy reader eviction/worker pool отсутствует;
- backup, unknown и user files normal recovery не удаляет.
