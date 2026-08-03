# ForgeDB

ForgeDB is a production-oriented embedded key-value storage engine being built in Rust.
The project prioritizes correctness, explicit durability contracts, bounded resource use,
versioned persisted formats, and corruption detection before performance optimization.

## Current milestone: v0.0.1 — Foundation

This release contains the non-storage foundation only:

- a Rust 2024 Cargo workspace with a Rust 1.89 MSRV;
- the public error model and validated database options;
- `DatabaseId`, `SequenceNumber`, `ColumnFamilyId`, and `ValueType`;
- logical internal-key ordering plus a bounded versioned codec;
- atomic in-memory `WriteBatch` construction with checked size accounting;
- a filesystem abstraction and native implementation;
- process-level exclusive database locking using operating-system file locks;
- a versioned `IDENTITY` file with file and directory synchronization;
- Linux and Windows CI, architecture decisions, and invariant documentation.

ForgeDB **does not yet provide a database open, put, get, WAL, memtable, SSTable, recovery,
or durability implementation**. Those claims begin only when the corresponding milestones are
implemented and tested.

## Workspace

```text
crates/forgedb       Public API types and configuration
crates/forgedb-core  Storage-neutral primitives, codecs, limits, and batches
crates/forgedb-fs    Filesystem abstraction, native locking, and database identity
```

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md),
[`docs/INVARIANTS.md`](docs/INVARIANTS.md), and the ADRs under [`docs/adr`](docs/adr).

## Roadmap

The next milestone is `v0.1.0 — Durable WAL Core`: WAL V1, mutable memtable, ordered writes,
atomic write batches, sync/no-sync durability, recovery, and corruption handling.

## License

MIT.
