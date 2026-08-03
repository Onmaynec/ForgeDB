# Архитектура ForgeDB v0.1.0

## Границы компонентов

```text
forgedb (публичный API, Database, Options, memtable, write orchestration)
  ├── forgedb-core (ошибки, идентификаторы, internal keys, Limits, WriteBatch)
  ├── forgedb-fs   (FileSystem, native I/O, LOCK, IDENTITY)
  │      └── forgedb-core
  └── forgedb-wal  (WAL V1, CRC32C, writer, replay)
         ├── forgedb-core
         └── forgedb-fs

forgedb-cli
  └── forgedb
```

Dependency direction односторонняя:

- `forgedb-core` не знает о файлах, WAL и публичной orchestration;
- `forgedb-fs` не содержит storage-engine policy;
- `forgedb-wal` владеет persisted-протоколом WAL, но не memtable и публичным API;
- `forgedb` связывает проверенные слои и является обычной зависимостью приложения;
- CLI остаётся тонким adapter над публичным crate.

## Состояние Database

`Database` владеет:

- путём каталога;
- стабильным `DatabaseId`;
- проверенными `Options`;
- флагом закрытия;
- `Mutex<DatabaseState>`.

`DatabaseState` содержит:

- mutable memtable;
- optional WAL writer для read-write режима;
- следующий sequence number;
- fatal write error;
- optional native lock guard, освобождаемый в `close()`.

## Thread model

В v0.1.0 все point reads и writes используют один mutex. Это гарантирует:

- единый порядок sequence assignment;
- отсутствие видимости частичного batch;
- простую связь между WAL order и memtable order;
- безопасное закрытие writer и lock.

Background workers отсутствуют. Group commit, immutable memtable и flush будут добавлены только вместе с отдельными
инвариантами и crash tests.

## Write path

```text
API validation
→ захват writer mutex
→ sequence reservation
→ memtable preflight
→ WAL frame append
→ optional sync
→ атомарное применение к memtable
→ публикация next_sequence
```

WAL error не приводит к memtable mutation и переводит writer в fatal state.

## Read path

Point lookup читает только mutable memtable. Memtable содержит runtime writes и результат WAL replay. Tombstone
представлен отсутствующим value. SSTable sources и merge iterator пока отсутствуют.

## Recovery

Recovery проверяет `IDENTITY`, заголовок WAL и каждый полный кадр. Batch полностью декодируется и проходит memtable
preflight до применения. Только последний физически неполный кадр может быть отброшен; mid-file corruption fatal.

## Ownership

- Идентификаторы, internal keys и batch владеют своими байтами.
- WAL decoder владеет bounded payload только после проверки длины.
- Memtable владеет ключами и последними значениями/tombstones.
- Native lock guard владеет системным file handle.
- `close()` синхронизирует writer, затем освобождает writer и lock.

## Persisted-файлы v0.1.0

```text
LOCK              системная process-level блокировка
IDENTITY          DatabaseId, формат V1
WAL-000001.log    WAL V1
```

SSTable, MANIFEST, CURRENT и OPTIONS snapshot пока отсутствуют.
