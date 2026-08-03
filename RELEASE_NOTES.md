# ForgeDB v0.0.1 — Foundation

ForgeDB now has a small, tested foundation for the future storage engine.

Highlights:

- Rust 2024 workspace with a Rust 1.89 MSRV;
- bounded core primitives and Internal Key V1 codec;
- checked atomic `WriteBatch` construction;
- validated database configuration;
- filesystem abstraction, native OS file locking, and versioned database identity;
- Linux and Windows CI plus architecture, invariant, and ADR documentation.

This release intentionally contains no WAL, SSTable, memtable, or database read/write path. It makes
no durability or crash-recovery claims beyond the explicitly documented identity-file installation
and lock behavior.
