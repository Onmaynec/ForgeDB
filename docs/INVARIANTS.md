# ForgeDB Invariants

Status values: **enforced** means code and tests exist in v0.0.1; **planned** identifies the milestone
that must enforce the invariant before claiming that feature.

| ID | Invariant | Status | Evidence |
|---|---|---|---|
| I-001 | One database directory has at most one read-write process. | enforced | `forgedb-fs::native::tests::second_exclusive_lock_is_rejected` |
| I-002 | A file number is never reused. | planned v0.2.0 | MANIFEST/version-set tests |
| I-003 | A sequence number never decreases or repeats. | primitive enforced; allocation planned v0.1.0 | `sequence_rejects_zero_and_overflow` |
| I-004 | An acknowledged WriteBatch is either fully recovered or fully absent. | planned v0.1.0 | WAL crash tests |
| I-005 | Visible sequence is published only after full batch application. | planned v0.1.0 | writer/recovery tests |
| I-006 | MANIFEST never references an incompletely installed SSTable. | planned v0.2.0 | installation crash matrix |
| I-007 | SSTables are immutable after installation. | planned v0.2.0 | SST lifecycle tests |
| I-008 | L1+ contains no overlapping file ranges. | planned v0.3.0 | VersionSet validation |
| I-009 | L0 point lookup checks files newest to oldest. | planned v0.3.0 | read-path tests |
| I-010 | A snapshot never observes a sequence above its own sequence. | planned v0.3.0 | snapshot model tests |
| I-011 | Compaction never drops a version visible to an active snapshot. | planned v0.4.0 | compaction model tests |
| I-012 | Tombstones are dropped only when proven safe. | planned v0.4.0 | bottommost/snapshot tests |
| I-013 | Obsolete files remain while used by a Version, reader, compaction, or checkpoint. | planned v0.4.0 | lifetime tests |
| I-014 | Every disk length and offset is checked before allocation or I/O. | enforced for Internal Key V1 and IDENTITY | codec and identity corruption tests |
| I-015 | Unknown future persisted versions are not opened as current. | enforced for Internal Key V1 and IDENTITY | future-version rejection tests |
| I-016 | Verify never changes a database. | planned v0.3.0 | verify inventory tests |
| I-017 | Salvage never changes the source database. | planned v0.9.0 | salvage integration tests |
| I-018 | Normal recovery never resumes after mid-file corruption. | planned v0.1.0 | WAL corruption tests |
| I-019 | Every live storage file UUID matches IDENTITY. | identity primitive enforced; storage files planned | identity round-trip tests |
| I-020 | Background storage failure is visible to later writes. | planned v0.2.0 | flush failure tests |
