# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust. Проект ставит корректность, явные гарантии durability, bounded resource usage, версионируемые persisted-форматы и обнаружение повреждений выше преждевременной оптимизации.

## Статус версий

- последняя опубликованная версия: **v0.3.0 — Full LSM Read Path**, GitHub Pre-release;
- следующий milestone разработки: **v0.4.0 — Leveled Compaction**.

## Возможности v0.3.0

- WAL V1, atomic WriteBatch и durability modes;
- SSTable V1, MANIFEST V1 и crash-safe CURRENT;
- mutable и immutable MVCC memtables;
- snapshot-aware point reads;
- cheap snapshots с registry активных sequence numbers;
- immutable `VersionSet`;
- L0 newest-first lookup;
- validated L1+ model с sorted/non-overlap invariant;
- forward merge iterator на binary heap;
- block-at-a-time SSTable cursors;
- bounded-memory range scan с bounds и limit;
- file inventory;
- неразрушающий recovery plan;
- quick/deep verify;
- CLI `scan`, `inventory`, `verify` и `inspect recovery-plan`.

Persisted-форматы не менялись: IDENTITY, Internal Key, WAL, SSTable, MANIFEST и CURRENT остаются V1.

## Пример API

```rust
use forgedb::{Database, Options, ReadOptions, Result, WriteOptions};

fn main() -> Result<()> {
    let database = Database::open("./data", Options::default())?;
    database.put(b"user:42", b"Alice", WriteOptions::sync())?;

    let snapshot = database.snapshot()?;
    database.put(b"user:42", b"Bob", WriteOptions::sync())?;

    assert_eq!(
        database.get_with_options(
            b"user:42",
            ReadOptions::from_snapshot(&snapshot),
        )?,
        Some(b"Alice".to_vec()),
    );

    for item in database.scan(
        b"user:".to_vec()..b"user;".to_vec(),
        ReadOptions::default(),
    )? {
        let (key, value) = item?;
        println!(
            "{} = {}",
            String::from_utf8_lossy(&key),
            String::from_utf8_lossy(&value),
        );
    }

    database.close()?;
    Ok(())
}
```

## CLI

```bash
cargo run -p forgedb-cli -- put ./data key value
cargo run -p forgedb-cli -- get ./data key
cargo run -p forgedb-cli -- scan ./data
cargo run -p forgedb-cli -- inventory ./data
cargo run -p forgedb-cli -- verify ./data --deep
cargo run -p forgedb-cli -- inspect recovery-plan ./data
```

## Гарантии recovery

- WAL-first bootstrap;
- generation-safe recovery current/previous WAL;
- current и previous MANIFEST-owned WAL проверяются до replay/open;
- отсутствующий MANIFEST-owned WAL блокирует open как corruption;
- SSTable становится live только через durable MANIFEST installation;
- obsolete previous WAL удаляется только после installation edit;
- orphan SST не включается автоматически в live state;
- unknown, backup и user files normal recovery не удаляет.

Точная crash matrix SST installation: [`docs/CRASH_RECOVERY_MATRIX_V0.2.md`](docs/CRASH_RECOVERY_MATRIX_V0.2.md).

## Структура workspace

```text
crates/forgedb           Database, MVCC, snapshots, iterators, inventory и verify
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

Обязательная matrix: Ubuntu stable, Windows stable и Rust 1.89.

## Следующий milestone: v0.4.0

Планируется leveled compaction:

- compaction picker и score;
- L0 → L1 и Ln → Ln+1;
- bounded-memory merge;
- snapshot-aware version retention;
- безопасные tombstone rules;
- output splitting;
- write stalls;
- obsolete file lifetime;
- manual compact.

## Ограничения v0.3.0

- физические L1+ файлы появятся только с compaction в v0.4.0;
- reverse iterator отсутствует;
- transactions отсутствуют;
- flush остаётся синхронным;
- background compaction и write stalls ещё не реализованы;
- normal recovery не удаляет произвольные orphan/unknown files.

## Документация

- [`docs/LSM_READ_PATH_V1.md`](docs/LSM_READ_PATH_V1.md);
- [`docs/SNAPSHOTS_AND_MVCC.md`](docs/SNAPSHOTS_AND_MVCC.md);
- [`docs/FILE_INVENTORY_AND_VERIFY.md`](docs/FILE_INVENTORY_AND_VERIFY.md);
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md);
- [`docs/releases/V0.3.0.md`](docs/releases/V0.3.0.md);
- [`docs/releases/V0.2.0.md`](docs/releases/V0.2.0.md).

## Лицензия

MIT.
