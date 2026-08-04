# ForgeDB

ForgeDB — embedded key-value storage engine на Rust с versioned formats, explicit durability, bounded resources и strict corruption detection.

## v0.3.0 draft

Реализованы WAL/SSTable/MANIFEST V1, MVCC, snapshots, L0 flush, immutable VersionSet, cursor range scan, inventory, recovery plan и verify.

```rust
use forgedb::{Database, Options, ReadOptions, Result, WriteOptions};

fn main() -> Result<()> {
    let db = Database::open("./data", Options::default())?;
    db.put(b"key", b"old", WriteOptions::sync())?;
    let snapshot = db.snapshot()?;
    db.put(b"key", b"new", WriteOptions::sync())?;
    db.flush()?;
    assert_eq!(db.get_with_options(b"key", ReadOptions::from_snapshot(&snapshot))?, Some(b"old".to_vec()));
    db.close()
}
```

Range scan использует binary heap. Memtables представлены frozen vectors; каждый SSTable source удерживает decoded index и один current data block.

## CLI

```bash
cargo run -p forgedb-cli -- put ./data key value --sync
cargo run -p forgedb-cli -- get ./data key
cargo run -p forgedb-cli -- scan ./data [start] [end-exclusive]
cargo run -p forgedb-cli -- inventory ./data
cargo run -p forgedb-cli -- inspect recovery-plan ./data
cargo run -p forgedb-cli -- verify ./data --deep
```

## Release gate

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

Run `30878893688` завершил Ubuntu stable, Windows stable и Rust 1.89 до checkout с `steps=null`; локальная toolchain недоступна. Merge, tag и release заблокированы.

До выпуска также нужно объединить временный streaming wrapper с storage core без изменения API.

Документация: [`docs`](docs). Лицензия: MIT.
