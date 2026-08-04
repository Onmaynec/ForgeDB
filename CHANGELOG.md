# История изменений

## [0.3.0] — stacked draft

MVCC snapshots, immutable VersionSet, cursor-based range scan, block-at-a-time SST reads, WAL recovery, retry-safe flush, inventory, recovery plan, verify and CLI. Persisted formats remain V1.

Functional tests are present but unverified because GitHub Actions jobs end before checkout. Streaming wrapper consolidation, full compiler/Clippy/test matrix and v0.2.0 release remain required.

## [0.2.0] — draft

SSTable V1, MANIFEST V1, CURRENT V1, numbered WAL generations and two-phase L0 flush.

## [0.1.0] — 2026-08-03

Durable WAL V1, mutable memtable, atomic WriteBatch, public point API and recovery.

## [0.0.1] — 2026-08-03

Workspace foundation, core types, Internal Key V1, IDENTITY V1 and filesystem abstraction.
