# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust. Проект ставит корректность, явные гарантии durability, bounded resource usage, версионируемые persisted-форматы и обнаружение повреждений выше преждевременной оптимизации.

## Статус версий

- текущий выпуск: **v0.8.2 — Streaming SST Handle Pool**, GitHub Pre-release;
- предыдущая версия: **v0.8.1 — SST Handle Cache & Reliability**;
- следующие задачи: **#31 — управление SST file handles, #35 — unified Column Families**.

## Возможности v0.8.2

- WAL V1, atomic WriteBatch и durability modes;
- SSTable V1 с checksums, blocks, index и Bloom filter;
- MANIFEST V2 с persisted L1+ metadata и чтением MANIFEST V1;
- crash-safe V1 → V2 MANIFEST migration при writable open;
- mutable/immutable MVCC memtables;
- cheap snapshots и snapshot-aware point reads;
- immutable `VersionSet`;
- L0 newest-first lookup и non-overlapping L1+;
- forward merge iterator и block-at-a-time SSTable cursors;
- bounded-memory range scan с bounds и limit;
- L0 → L1 и Ln → Ln+1 leveled compaction;
- MVCC-aware retention и conservative bottommost tombstone dropping;
- output splitting по user-key boundaries;
- L0 slowdown/stop backpressure;
- automatic compaction после `flush()`;
- optimistic snapshot-isolation transactions;
- own-write visibility внутри transaction;
- write-write conflict detection по committed sequence numbers;
- atomic transaction commit через существующий WAL `WriteBatch` path;
- transaction rollback и terminal state machine;
- индексный SSTable point lookup с Bloom filter, поддерживающий MVCC-версии между blocks;
- bounded LRU для декодированных data blocks, counters через `cache_stats()`;
- бинарный поиск по non-overlapping L1+ ranges;
- reference-aware manual cleanup доказанных retired SSTables после compaction;
- public `compact_range`, file inventory, recovery plan и quick/deep verify;
- CLI transaction/scan/compact/inventory/verify/inspect;
- повторяемый `close()`: writer handles и LOCK удерживаются до успешного sync WAL/MANIFEST;
- однократный synchronous flush/retry для `put/delete/write` при переполнении mutable memtable;
- best-effort reference-aware SST cleanup после успешной compaction (в рамках текущего open handle).
- lazy random-access SST reader: header/footer и служебные blocks проверяются при открытии, data blocks читаются при запросе с CRC32C;
- measurement throughput и read latency p50/p95/p99 в `point_lookup_bench`.

## Column Families (v0.7.0)

Добавлен новый публичный фасад `ColumnFamilies` для именованных хранилищ с отдельными WAL/LSM каталогами. Default остаётся в корне старой базы, поэтому данные v0.6.2 доступны без миграции.

```rust
use forgedb::{ColumnFamilies, ColumnFamilyId, Options, Result, WriteOptions};

fn main() -> Result<()> {
    let database = ColumnFamilies::open("./data", Options::default())?;
    let users = database.create("users")?;
    database.put(users, b"42", b"Alice", WriteOptions::sync())?;
    assert_eq!(database.get(users, b"42")?, Some(b"Alice".to_vec()));
    assert_eq!(database.get(ColumnFamilyId::DEFAULT, b"42")?, None);
    database.close()
}
```

`list()` возвращает зарегистрированные ID для следующих открытий. `drop_family(id)` логически удаляет family, но не уничтожает физические файлы без отдельного безопасного reaper. `write(id, batch)` и `begin_transaction(id)` атомарны **только внутри одной family**. У каждой family отдельная WAL sequence timeline; глобальных mixed-family atomic batches и snapshots нет. В legacy `WriteBatch::put_cf` ненулевой ID пока отклоняется намеренно — этот API не реализует общий WAL. Для записей используйте `ColumnFamilies::write(id, обычный WriteBatch)`.

Подробнее: [v0.7.0](docs/releases/V0.7.0.md), [CF_REGISTRY V1](docs/formats/CF_REGISTRY_V1.md) и [ADR 0015](docs/adr/0015-column-family-isolation.md).

В CLI v0.7.0 доступны `forgedb cf create ./data users`, `forgedb cf list ./data`, `forgedb cf put ./data 1 key value`, `forgedb cf get ./data 1 key` и `cf scan/delete/drop`. Для named CF `cf list` возвращает числовые ID.

## FQL V1 и интерактивный shell (v0.8.0)

FQL — строгий командный язык над key-value базой (это **не SQL**). Он поддерживает `GET`, `PUT`, `DELETE`, `SCAN`, управление Column Families и `BEGIN/COMMIT/ROLLBACK` внутри выбранной family.

```powershell
.\forgedb.exe fql .\data "PUT account 'Alice Smith'"
.\forgedb.exe fql .\data "GET account" --read-only
.\forgedb.exe shell .\data
.\forgedb.exe shell .\data --file .\queries.fql
```

У FQL одна команда на строку, допустима завершающая точка с запятой. Значения могут содержать бинарные bytes (quoted escapes, `X'HEX'`). `SCAN` возвращает максимум 100 строк по умолчанию, 10 000 по явному LIMIT. В shell при ошибке интерпретатор продолжает работу, в файле останавливается на первой ошибке; незавершённый BEGIN при выходе откатывается. **Файл скрипта не атомарен целиком**, ранее зафиксированные строки остаются durable.

Руководство: [FQL V1](docs/FQL.md), [ADR 0016](docs/adr/0016-fql-cli.md), [v0.8.0](docs/releases/V0.8.0.md).

## CLI без компиляции

Для Windows x86-64 и Linux x86-64 опубликованы архивы с готовым CLI и SHA-256 checksum в [GitHub Releases](https://github.com/Onmaynec/ForgeDB/releases). Скачать и распаковать `forgedb-v0.8.2-windows-x86_64.zip` или `forgedb-v0.8.2-linux-x86_64.tar.gz`.

Пример первого запуска после распаковки (Linux; в PowerShell используйте `./forgedb.exe`):

```bash
./forgedb put ./data hello world
./forgedb get ./data hello
./forgedb scan ./data
./forgedb verify ./data --deep
```

Подробности и проверка контрольной суммы: [docs/QUICKSTART.md](docs/QUICKSTART.md).

## Пример API

```rust
use forgedb::{Database, Options, Result, WriteOptions};

fn main() -> Result<()> {
    let database = Database::open("./data", Options::default())?;
    database.put(b"balance:42", b"100", WriteOptions::sync())?;

    let mut transaction = database.begin_transaction()?;
    assert_eq!(transaction.get(b"balance:42")?, Some(b"100".to_vec()));

    transaction.put(b"balance:42", b"150")?;
    transaction.delete(b"pending:42")?;

    // Own write виден transaction до commit, но ещё не опубликован другим readers.
    assert_eq!(transaction.get(b"balance:42")?, Some(b"150".to_vec()));
    transaction.commit(WriteOptions::sync())?;

    assert_eq!(database.get(b"balance:42")?, Some(b"150".to_vec()));
    database.close()?;
    Ok(())
}
```

## SST Handle Cache v0.8.1

Для point reads добавлен общий LRU кэш native дескрипторов SST. `Options::table_cache_capacity` теперь ограничивает количество открытых **точечных SST readers** в одном `Database` (по умолчанию 512). После eviction файл открывается снова с проверкой physical size, DB identity, file number, sequence range и header checksum; data block CRC32C сохранён.

```rust
let stats = database.table_handle_stats()?;
println!("open {}/{}; evictions {}, reopens {}",
    stats.open_handles, stats.capacity, stats.evictions, stats.reopens);
```

Отдельный `block_cache_size` продолжает ограничивать decoded-block residency. **Это не общий process FD budget:** streaming iterators, compaction, WAL/MANIFEST/LOCK и другие named Column Families используют отдельные handles. Issue #31 остаётся открытым для полной политики. При удалении retired SST pooled FD закрывается до физического удаления, включая Windows.

Подробнее: [v0.8.1](docs/releases/V0.8.1.md), [ADR 0017](docs/adr/0017-bounded-sst-handle-cache.md), [Options](docs/OPTIONS.md).

## Потоковое чтение через общий SST FD pool (v0.8.2)

Потоковый `Database::scan` и входные SST compaction теперь используют тот же `TableHandlePool`, что и точечный `GET`. После проверки header/footer/index/filter/properties и первого data block cursor закрывает исходный `File`: при последующих блоках FD берётся из общего LRU, а не удерживается на срок жизни итератора.

`Options::table_cache_capacity` ограничивает **resident handles общего SST read pool одного Database**. Проверки размера/ID/sequence/header при reopen и CRC32C data blocks сохраняются. Старые iterators удерживают `VersionSet` и защищают SST от premature cleanup.

Это не общий FD limit процесса: при начальной проверке cursor кратковременно открывает вне pool дополнительный FD; SST output, WAL/MANIFEST/LOCK, deep verify и named child databases — отдельные источники дескрипторов. Полное решение Issue #31 остаётся открытым. Обязательные CI тесты: 48 SST × 10 keys, concurrent GET+scan и compaction cleanup; дополнительный стресс с 1024 SST помечен `#[ignore]`.

Документы: [v0.8.2](docs/releases/V0.8.2.md), [ADR 0018](docs/adr/0018-streaming-sst-handle-leases.md).

## Cache and Performance

`Options::block_cache_size` ограничивает суммарный учитываемый объём **декодированных SST data blocks**; `0` отключает кеширование. `Database::cache_stats()` показывает hits, misses, evictions, bypasses и текущую residency кэша. SST reader использует Bloom filter и индекс, не материализуя всю таблицу при каждом point read или проверке конфликта транзакции.

`Database::cleanup_obsolete_sstables()` консервативно удаляет только файлы, исключённые из MANIFEST успешной compaction в текущем открытом процессе, и ждёт освобождения их reader handles. В v0.6.1 вызывается также best-effort очистка после успешной compaction. Ошибка удаления не превращает уже завершённую compaction в неуспешную запись; для диагностики и повторной очистки остаётся явный метод. После restart непроверенные orphan-файлы не удаляются.

**Граница v0.6.2:** `TableReader::open` больше не загружает весь физический SST; reader сохраняет открытый random-access file handle и индекс/фильтр/свойства, а data blocks читает только при необходимости. `block_cache_size` ограничивает лишь учтённые декодированные blocks, а не общий RSS (временные blocks, index/filter, memtables и scan cursors остаются вне бюджета). `table_cache_capacity` пока не ограничивает количество открытых файловых handles. Range iterator использует независимый streaming scan.

Измерение без внешних зависимостей:

```bash
cargo run --locked --release -p forgedb --example point_lookup_bench -- 8192 50000
```

Контракт и ограничения: [`docs/CACHE_AND_PERFORMANCE.md`](docs/CACHE_AND_PERFORMANCE.md).

## Transactions

`Database::begin_transaction()` фиксирует snapshot текущего visible sequence. Reads внутри transaction видят собственные buffered mutations поверх этого snapshot.

Commit выполняет write-write validation: если любой ключ write set получил более новую committed version после snapshot transaction, commit целиком отклоняется как `Error::Conflict { keys }`. Validation и последующий WAL append сериализованы единым public write gate.

ForgeDB v0.5.0 предоставляет **snapshot isolation**, но не заявляет serializable isolation, predicate locking, phantom protection или защиту от write skew. Read set ограничен числом уникальных ключей (`max_batch_operations`) и суммарной длиной ключей (`max_batch_size`). До включения ключа в read set проверяется `max_key_size`. Read set не используется для full read-write conflict validation.

`WriteOptions::disable_wal()` для transaction commit запрещён. `Sync` и `NoSync` используют существующие WAL durability semantics.

Подробный контракт: [`docs/TRANSACTIONS.md`](docs/TRANSACTIONS.md).

## CLI

```bash
cargo run -p forgedb-cli -- put ./data key value
cargo run -p forgedb-cli -- get ./data key

cargo run -p forgedb-cli -- transaction ./data \
  --put user:1 Alice \
  --put user:2 Bob \
  --delete pending:1 \
  --sync

cargo run -p forgedb-cli -- tx ./data --get user:1 --put user:1 Carol --get user:1 --sync
cargo run -p forgedb-cli -- tx ./data --put temporary value --rollback

cargo run -p forgedb-cli -- scan ./data
cargo run -p forgedb-cli -- compact ./data
cargo run -p forgedb-cli -- compact ./data user:0000 user:9999
cargo run -p forgedb-cli -- inventory ./data
cargo run -p forgedb-cli -- verify ./data --deep
cargo run -p forgedb-cli -- inspect recovery-plan ./data
```

CLI v0.5.0 принимает текстовые ключи/значения как UTF-8. Library API остаётся binary-safe.

Transaction conflict использует exit code `9`. `--disable-wal` для transaction CLI отклоняется.

## Persisted compatibility

| Формат | Read | Write |
|---|---|---|
| IDENTITY | 1 | 1 |
| Internal Key | 1 | 1 |
| WAL | 1 | 1 |
| SSTable | 1 | 1 |
| MANIFEST | 1, 2 | 2 |
| CURRENT | 1 | 1 |

v0.5.0 не добавляет новый persisted transaction format. Atomic commit кодируется существующим WAL V1 frame для одного `WriteBatch`.

Writable open MANIFEST V1 не изменяет старый файл in-place: ForgeDB создаёт новый V2 snapshot MANIFEST и crash-safe публикует его через `CURRENT`. Read-only open V1 ничего не мигрирует.

## Compaction

Default policy:

```text
levels:                 7
L0 compaction trigger:  4 files
L0 slowdown trigger:    8 files
L0 stop trigger:        12 files
L1 target:              256 MiB
level multiplier:       10
target SST:             64 MiB
```

Compaction сохраняет newest version каждого user key и версии, необходимые oldest active snapshot. Активная transaction удерживает обычный зарегистрированный snapshot, поэтому её snapshot sequence участвует в retention decision.

Tombstone может быть отброшен только в bottommost range при snapshot-safe условиях. Output SSTables полностью создаются до atomic MANIFEST installation edit. File numbers резервируются durable edit до построения outputs и не переиспользуются после crash.

Подробности: [`docs/COMPACTION.md`](docs/COMPACTION.md).

## Recovery

- current/previous MANIFEST-owned WAL проверяются до replay;
- отсутствующий live WAL/SST является corruption;
- SST становится live только через durable MANIFEST installation;
- compaction до installation сохраняет inputs authoritative;
- compaction после synced installation делает outputs authoritative;
- orphan SST не включается автоматически в live state;
- unknown, backup и user files normal recovery не удаляет;
- future persisted versions отклоняются;
- committed transaction `Sync` восстанавливается как один обычный WAL batch, а не как отдельный transaction format.

## Структура workspace

```text
crates/forgedb           Database, MVCC, snapshots, transactions, reads, compaction и verify
crates/forgedb-cli       CLI put/get/delete/transaction/scan/compact/inventory/verify/inspect
crates/forgedb-core      Типы, limits, internal keys и WriteBatch
crates/forgedb-fs        Filesystem abstraction, native lock и identity
crates/forgedb-wal       WAL V1 и numbered generations
crates/forgedb-sstable   SSTable V1, blocks, Bloom filter и builder/reader
crates/forgedb-manifest  MANIFEST V1/V2, VersionEdit и CURRENT
```

## Release gate

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked --workspace --doc
cargo build --locked --workspace --release
```

Основная CI matrix: Ubuntu stable, Windows stable и Rust 1.89.

## Ограничения v0.8.2

- isolation — snapshot isolation, не serializable;
- обязательна write-write conflict detection, но full read-write validation пока отсутствует;
- predicate/range locks, phantom protection, nested transactions и savepoints отсутствуют;
- full FQL parser/shell остаётся milestone v0.8.0;
- compaction synchronous; background worker pool появится позже;
- compaction merge bounded `max_compaction_bytes`, но materialized внутри этого budget;
- retired SST очищаются автоматически после compaction и вручную по вызову при освобождении reader handles; кросс-рестартная GC отсутствует;
- auto-flush при заполнении memtable синхронен, однократен и применяется только к публичным `put/delete/write`; транзакционный commit пока использует прежний путь;
- `table_cache_capacity` и `max_background_flushes/max_background_compactions` пока зарезервированы и не управляют рабочими worker pools/reader eviction;
- `TableReader` хранит открытый file handle каждого live SST; `table_cache_capacity` пока не ограничивает число FD, общий RSS и временную память запросов не ограничивает `block_cache_size`;
- data blocks проверяются при обращении; индекс, фильтр, свойства и footer — при открытии; deep verify независимо проверяет полное содержимое;
- benchmark не задаёт фиксированных performance thresholds: результаты зависят от среды;
- reverse iterator и range tombstones отсутствуют;
- normal recovery не удаляет произвольные orphan/unknown files.

## Документация

- [`docs/releases/V0.8.2.md`](docs/releases/V0.8.2.md);
- [`docs/adr/0018-streaming-sst-handle-leases.md`](docs/adr/0018-streaming-sst-handle-leases.md);
- [`docs/releases/V0.8.1.md`](docs/releases/V0.8.1.md);
- [`docs/adr/0017-bounded-sst-handle-cache.md`](docs/adr/0017-bounded-sst-handle-cache.md);
- [`docs/FQL.md`](docs/FQL.md);
- [`docs/adr/0016-fql-cli.md`](docs/adr/0016-fql-cli.md);
- [`docs/releases/V0.8.0.md`](docs/releases/V0.8.0.md);
- [`docs/releases/V0.7.0.md`](docs/releases/V0.7.0.md);
- [`docs/formats/CF_REGISTRY_V1.md`](docs/formats/CF_REGISTRY_V1.md);
- [`docs/adr/0015-column-family-isolation.md`](docs/adr/0015-column-family-isolation.md);

- [`docs/CACHE_AND_PERFORMANCE.md`](docs/CACHE_AND_PERFORMANCE.md);
- [`docs/OPTIONS.md`](docs/OPTIONS.md);
- [`docs/releases/V0.6.2.md`](docs/releases/V0.6.2.md);
- [`docs/adr/0014-lazy-sst-readers.md`](docs/adr/0014-lazy-sst-readers.md);
- [`docs/releases/V0.6.1.md`](docs/releases/V0.6.1.md);
- [`docs/TRANSACTIONS.md`](docs/TRANSACTIONS.md);
- [`docs/COMPACTION.md`](docs/COMPACTION.md);
- [`docs/formats/MANIFEST_V2.md`](docs/formats/MANIFEST_V2.md);
- [`docs/LSM_READ_PATH_V1.md`](docs/LSM_READ_PATH_V1.md);
- [`docs/SNAPSHOTS_AND_MVCC.md`](docs/SNAPSHOTS_AND_MVCC.md);
- [`docs/FILE_INVENTORY_AND_VERIFY.md`](docs/FILE_INVENTORY_AND_VERIFY.md);
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md);
- [`docs/adr/0010-manifest-v2-leveled-files.md`](docs/adr/0010-manifest-v2-leveled-files.md);
- [`docs/adr/0011-optimistic-snapshot-isolation.md`](docs/adr/0011-optimistic-snapshot-isolation.md);
- [`docs/adr/0012-indexed-sst-lookup-and-cache.md`](docs/adr/0012-indexed-sst-lookup-and-cache.md);
- [`docs/adr/0013-reference-aware-sst-retirement.md`](docs/adr/0013-reference-aware-sst-retirement.md);
- [`docs/releases/V0.6.0.md`](docs/releases/V0.6.0.md);
- [`docs/releases/V0.5.0.md`](docs/releases/V0.5.0.md).

## Лицензия

MIT.
