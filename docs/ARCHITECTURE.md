# Архитектура ForgeDB v0.6.0

## Назначение

ForgeDB — встраиваемый key-value storage engine с LSM-структурой и собственными persisted-форматами. Порядок приоритетов: корректность записи и восстановления, проверяемые ограничения ресурсов, совместимость файлов и только затем производительность.

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

Публичный `Database` экспортирует `StreamingDatabase`. Пока используется композиция исходников `database_v3_fixed.rs`, `database_streaming_wrapper.rs` и `compaction.rs` через `include!`. Это действующая реализация v0.4.0; объединение модулей — отдельный рефакторинг, не условие чтения persisted-форматов.

Внутреннее состояние хранит:

- mutable MVCC memtable и необязательную pending immutable memtable;
- current WAL writer и writer MANIFEST;
- `ManifestState` и текущий `Arc<VersionSet>`;
- следующий и последний опубликованный sequence;
- признак fatal write failure и `flush_in_progress`;
- нативную эксклюзивную блокировку каталога.

`SnapshotRegistry` отслеживает живые snapshot handles и oldest active sequence. `closed` проверяется на входе в публичные операции.

## Путь записи

Операции `put`, `delete` и `write` направляются в общий `Database::write`. Под mutex выполняются validation, резервирование sequence range, preflight mutable memtable, WAL append и, для `Sync`, синхронизация. Только после успешной WAL boundary изменения публикуются в memtable.

`DisableWal` допускает потерю неподтверждённого на диске состояния при аварийном завершении. Полный контракт: [WRITE_PATH.md](WRITE_PATH.md) и [DURABILITY.md](DURABILITY.md).

## Путь чтения

Чтение учитывает текущий sequence или зарегистрированный snapshot. Источники: mutable memtable, pending immutable memtable, затем файлы актуального immutable `VersionSet` — L0 в порядке от новых к старым и non-overlapping L1+.

Публичный `scan` использует merge iterator и block-at-a-time SST cursors. Reader удерживает ссылку на выбранную версию LSM на время работы. Подробности: [READ_PATH.md](READ_PATH.md).

## Cache and indexed point lookup

`VersionSet` использует проверенный SSTable index/Bloom filter для snapshot-aware point reads и optimistic conflict validation. `BlockCache` держит только bounded декодированные data blocks; физический `TableReader` пока eagerly загружает SST bytes. Публичный `cache_stats()` показывает hits/misses/evictions. Range scan не пользуется point cache и удерживает `Arc<VersionSet>` до завершения.

Подробности: [CACHE_AND_PERFORMANCE.md](CACHE_AND_PERFORMANCE.md).

## Flush и compaction

Flush синхронизирует current WAL перед rotation, фиксирует новые номера файлов, создаёт immutable SSTable и публикует новую L0-топологию через synced MANIFEST edit. Recovery различает current и previous WAL поколения.

Leveled compaction выбирает L0→L1 или Ln→Ln+1, строит outputs и публикует их единым MANIFEST edit. До installation authoritative остаются inputs, после durable installation — outputs. Compaction синхронна; её входные SST не удаляются немедленно без reference-aware reaper. Правила: [COMPACTION.md](COMPACTION.md).

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

## Границы v0.4.0

- один writer path и эксклюзивный process lock; shared read-only opens не поддерживаются;
- compaction synchronous, её merge ограничен memory budget, но ещё не fully streaming;
- reverse iterator и transactions отсутствуют;
- manual process-local retired SST cleanup реализован, но restart-aware automatic GC отсутствует;
- backup, unknown и user files normal recovery не удаляет.
