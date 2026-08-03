# Changelog

All notable changes to ForgeDB are documented in this file.

The project follows Semantic Versioning once the public API and persisted formats become stable.
Pre-1.0 releases may change APIs, but persisted-format changes still require explicit versioning.

## [0.0.1] - 2026-08-03

### Added

- Cargo workspace with `forgedb`, `forgedb-core`, and `forgedb-fs` crates.
- Stable Rust and Edition 2024 configuration with shared lint policy.
- Public error model and validated `Options`.
- `DatabaseId`, `SequenceNumber`, `ColumnFamilyId`, and `ValueType` primitives.
- Logical internal-key comparator and bounded Internal Key V1 codec.
- `WriteBatch` with atomic mutation, configured limits, and checked size accounting.
- Filesystem trait and native implementation.
- Operating-system-backed exclusive database lock.
- Versioned, synchronized database `IDENTITY` file.
- Unit tests, deterministic property tests, Linux/Windows CI, documentation, and ADRs.

### Not included

- No WAL, memtable, SSTable, MANIFEST, database read/write API, or durability claim.
