# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust. Проект ставит корректность,
явные гарантии durability, ограниченное использование ресурсов, версионируемые persisted-форматы
и обнаружение повреждений выше преждевременной оптимизации производительности.

## Статус версий

- последняя опубликованная версия: **v0.2.0 — SSTable and Flush**, GitHub Pre-release;
- текущий milestone разработки: **v0.3.0 — Full LSM Read Path**, draft.

## Что добавляет v0.3.0

- MVCC memtable, сохраняющая версии internal keys;
- cheap snapshots с registry активных sequence;
- snapshot-aware `get` и `scan`;
- `ReadOptions`;
- immutable `VersionSet`;
- L0 newest-first lookup;
- validated модель L1+ с sorted/non-overlap invariant;
- forward merge iterator на binary heap;
- block-at-a-time SSTable cursors;
- range filtering и `limit`;
- file inventory;
- неразрушающий recovery plan;
- quick/deep verify;
- CLI `scan`, `inventory`, `verify` и `inspect recovery-plan`.

Persisted-форматы не меняются: IDENTITY, Internal Key, WAL, SSTable, MANIFEST и CURRENT остаются V1.

## Пример draft API v0.3.0

```rust
use forgedb::{Database, Options, ReadOptions, Result, WriteOptions};

fn main() -> Result<()> {
    let database = Database::open("./data", Options::default())?;
    database.put(b"user:42", b"Alice", WriteOptions::sync())?;
    let snapshot = database.snapshot()?;
    database.put(b"user:42", b"Bob", WriteOptions::sync())?;

    assert_eq!(
        database.get_with_options(b"user:42", ReadOptions::from_snapshot(&snapshot))?,
        Some(b"Alice".to_vec())
    );

    for item in database.scan(b"user:".to_vec()..b"user;".to_vec(), ReadOptions::default())? {
        let (key, value) = item?;
        println!("{} = {}", String::from_utf8_lossy(&key), String::from_utf8_lossy(&value));
    }

    database.close()?;
    Ok(())
}
```

## CLI

```bash
cargo run -p forgedb-cli -- scan ./data
cargo run -p forgedb-cli -- inventory ./data
cargo run -p forgedb-cli -- verify ./data --deep
cargo run -p forgedb-cli -- inspect recovery-plan ./data
```

## Гарантии v0.2.0, сохраняемые в v0.3

- WAL-first bootstrap;
- generation-safe recovery current/previous WAL;
- SSTable становится live только через durable MANIFEST installation;
- obsolete previous WAL удаляется только после installation edit;
- missing MANIFEST-owned file блокирует open;
- unknown/user files normal recovery не удаляет.

Точная crash matrix: [`docs/CRASH_RECOVERY_MATRIX_V0.2.md`](docs/CRASH_RECOVERY_MATRIX_V0.2.md).

## Структура workspace

```text
crates/forgedb           Database, MVCC, snapshots, iterator, inventory и verify
crates/forgedb-cli       CLI put/get/delete/scan/inventory/verify/inspect
crates/forgedb-core      Типы, лимиты, internal keys и WriteBatch
crates/forgedb-fs        Файловая абстракция, native lock и identity
crates/forgedb-wal       WAL V1 и numbered generations
crates/forgedb-sstable   SSTable V1, blocks, Bloom filter и TableBuildRequest
crates/forgedb-manifest  MANIFEST V1, VersionEdit и CURRENT
```

## Проверка

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked --workspace --doc
cargo build --locked --workspace --release
```

Обязательная matrix: Ubuntu stable, Windows stable и Rust 1.89. До полного зелёного gate `v0.3.0` остаётся draft.

## Ограничения draft v0.3.0

- физические L1+ файлы появятся только с compaction в v0.4.0;
- нет reverse iterator;
- нет transactions;
- flush остаётся синхронным;
- normal recovery не удаляет произвольные orphan/unknown files.

## Документация

- [`docs/LSM_READ_PATH_V1.md`](docs/LSM_READ_PATH_V1.md);
- [`docs/SNAPSHOTS_AND_MVCC.md`](docs/SNAPSHOTS_AND_MVCC.md);
- [`docs/FILE_INVENTORY_AND_VERIFY.md`](docs/FILE_INVENTORY_AND_VERIFY.md);
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md);
- [`docs/releases/V0.3.0_DRAFT.md`](docs/releases/V0.3.0_DRAFT.md);
- [`docs/releases/V0.2.0.md`](docs/releases/V0.2.0.md).

## Лицензия

MIT.
