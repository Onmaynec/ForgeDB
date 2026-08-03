# ADR 0001: Build a native LSM storage engine

- Status: Accepted
- Date: 2026-08-03

ForgeDB will implement its own WAL, memtable, SSTable, MANIFEST, recovery, and compaction layers.
Existing databases may be studied but cannot be used as the storage backend.

This keeps the project aligned with its purpose but makes correctness, crash testing, compatibility,
and operational documentation mandatory parts of every milestone.
