# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust с явными durability guarantees, bounded resource use, versioned persisted formats и strict corruption detection.

## Текущий этап: v0.3.0 — Full LSM Read Path

`v0.3.0` — stacked draft поверх ещё не выпущенной `v0.2.0`. Реализованы WAL/SSTable/MANIFEST V1, MVCC memtables, двухфазный L0 flush, immutable `VersionSet`, snapshots, point reads, cursor-based range scan, inventory, recovery plan и verify.

Streaming range path использует binary heap. Memtables представлены `VecEntryCursor`, а каждый SSTable source — `SstEntryCursor`, который удерживает decoded index и только один текущий data block. Код и tests присутствуют, но compiler/CI результат пока отсутствует.

## Пример

```rust
use forgedb::{Database, Options, ReadOptions, Result, WriteOptions};

fn main() -> Result<()> {
    let database = Database::open("./data", Options::default())?;
    database.put(b"user:42", b"Alice", WriteOptions::sync())?;
    let snapshot = database.snapshot()?;
    database.put(b"user:42", b"Alice v2", WriteOptions::sync())?;
    database.flush()?;

    assert_eq!(
        database.get_with_options(b"user:42", ReadOptions::from_snapshot(&snapshot))?,
        Some(b"Alice".to_vec()),
    );

    for row in database.scan(b"user:".to_vec()..b"user;".to_vec(), ReadOptions::default())? {
        let (key, value) = row?;
        println!("{} = {}", String::from_utf8_lossy(&key), String::from_utf8_lossy(&value));
    }
    database.close()
}
```

## CLI

```bash
cargo run -p forgedb-cli -- put ./data key value --sync
cargo run -p forgedb-cli -- get ./data key
cargo run -p forgedb-cli -- delete ./data key --sync
cargo run -p forgedb-cli -- scan ./data [start] [end-exclusive]
cargo run -p forgedb-cli -- inventory ./data
cargo run -p forgedb-cli -- inspect recovery-plan ./data
cargo run -p forgedb-cli -- verify ./data --deep
```

`inspect recovery-plan` работает под process lock, но не выполняет rename/delete.

## Release gate

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

Нужны Ubuntu stable, Windows stable и Rust 1.89. GitHub Actions run `30878893688` завершил все jobs до checkout с `steps=null`; локальная toolchain недоступна. Merge, tag и release заблокированы.

## Ограничения

Нет compaction, L1+ data, block/table cache, backward iterator, transactions и column-family API. Index выбранного SSTable загружается целиком. Streaming `Database` временно оформлен wrapper-ом над storage core и должен быть объединён после доступного compiler/refactor pass без изменения API.

## Документация

- [`docs/LSM_READ_PATH_V1.md`](docs/LSM_READ_PATH_V1.md)
- [`docs/SNAPSHOTS_AND_MVCC.md`](docs/SNAPSHOTS_AND_MVCC.md)
- [`docs/FILE_INVENTORY_AND_VERIFY.md`](docs/FILE_INVENTORY_AND_VERIFY.md)
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md)
- [`docs/formats`](docs/formats)

## Лицензия

MIT.
