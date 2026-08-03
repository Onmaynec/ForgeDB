# ForgeDB v0.0.1 — Foundation

ForgeDB now has a small, tested foundation for the future storage engine.

## Highlights

- Rust 2024 workspace with a Rust 1.89 MSRV;
- bounded core primitives and Internal Key V1 codec;
- checked atomic `WriteBatch` construction;
- validated database configuration;
- filesystem abstraction, native OS file locking, and versioned database identity;
- architecture, invariant, durability, validation, and ADR documentation.

## Validation

The release candidate passed GitHub Actions run `30794747746`:

- Ubuntu stable: rustfmt, Clippy with warnings denied, workspace tests, doctests, and release build;
- Windows stable: rustfmt, Clippy with warnings denied, workspace tests, doctests, and release build;
- Ubuntu Rust 1.89: Clippy with warnings denied and workspace tests.

## Scope boundary

This release intentionally contains no WAL, SSTable, memtable, MANIFEST, database read/write path,
or crash recovery. It makes no data-durability claim beyond the explicitly documented identity-file
installation and process-level lock behavior.
