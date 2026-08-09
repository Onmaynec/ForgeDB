# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust. Проект ставит корректность, явные гарантии durability, bounded resource usage, версионируемые persisted-форматы и обнаружение повреждений выше преждевременной оптимизации.

## Статус версий

- текущий milestone: **v0.5.0 — MVCC Transactions**;
- следующий milestone: **v0.6.0 — Cache and Performance**.

## Возможности v0.5.0

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
- public `compact_range`, file inventory, recovery plan и quick/deep verify;
- CLI transaction/scan/compact/inventory/verify/inspect.

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

## Transactions

`Database::begin_transaction()` фиксирует snapshot текущего visible sequence. Reads внутри transaction видят собственные buffered mutations поверх этого snapshot.

Commit выполняет write-write validation: если любой ключ write set получил более новую committed version после snapshot transaction, commit целиком отклоняется как `Error::Conflict { keys }`. Validation и последующий WAL append сериализованы единым public write gate.

ForgeDB v0.5.0 предоставляет **snapshot isolation**, но не заявляет serializable isolation, predicate locking, phantom protection или защиту от write skew. Read set bounded и отслеживается, но full read-write conflict validation пока не выполняется.

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

## Ограничения v0.5.0

- isolation — snapshot isolation, не serializable;
- обязательна write-write conflict detection, но full read-write validation пока отсутствует;
- predicate/range locks, phantom protection, nested transactions и savepoints отсутствуют;
- full FQL parser/shell остаётся milestone v0.8.0;
- compaction synchronous; background worker pool появится позже;
- compaction merge bounded `max_compaction_bytes`, но materialized внутри этого budget;
- input SST после compaction пока не удаляются автоматически: reference-aware Windows-safe obsolete reaper ещё не реализован;
- reverse iterator и range tombstones отсутствуют;
- normal recovery не удаляет произвольные orphan/unknown files.

## Документация

- [`docs/TRANSACTIONS.md`](docs/TRANSACTIONS.md);
- [`docs/COMPACTION.md`](docs/COMPACTION.md);
- [`docs/formats/MANIFEST_V2.md`](docs/formats/MANIFEST_V2.md);
- [`docs/LSM_READ_PATH_V1.md`](docs/LSM_READ_PATH_V1.md);
- [`docs/SNAPSHOTS_AND_MVCC.md`](docs/SNAPSHOTS_AND_MVCC.md);
- [`docs/FILE_INVENTORY_AND_VERIFY.md`](docs/FILE_INVENTORY_AND_VERIFY.md);
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md);
- [`docs/adr/0010-manifest-v2-leveled-files.md`](docs/adr/0010-manifest-v2-leveled-files.md);
- [`docs/adr/0011-optimistic-snapshot-isolation.md`](docs/adr/0011-optimistic-snapshot-isolation.md);
- [`docs/releases/V0.5.0.md`](docs/releases/V0.5.0.md).

## Лицензия

MIT.
