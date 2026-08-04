# ForgeDB

Embedded Rust key-value storage engine with versioned formats, explicit durability and strict corruption detection.

`v0.3.0` is a stacked draft over unreleased `v0.2.0`. It implements WAL/SSTable/MANIFEST V1, MVCC snapshots, L0 flush, immutable VersionSet, cursor range scan, inventory, recovery plan and verify.

SSTable range sources keep a decoded index and one current data block. Tests cover multi-block and multi-file scans, but CI is unverified: run `30878893688` ended before checkout with `steps=null` on Ubuntu stable, Windows stable and Rust 1.89.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

Merge, tag and release remain blocked. Documentation is under [`docs`](docs). MIT.
