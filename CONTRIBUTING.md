# Contributing to ForgeDB

ForgeDB values correctness and recoverability over benchmark results.

Before submitting a change:

1. Describe the invariant or contract affected by the change.
2. Add unit and integration coverage, including malformed inputs where relevant.
3. Document crash-consistency and persisted-format impact.
4. Run formatting, Clippy, tests, doctests, and a release build.
5. Do not introduce a storage backend such as SQLite, RocksDB, LevelDB, sled, or redb.
6. Do not add `unsafe`, `todo!`, `unimplemented!`, or runtime `unwrap`/`expect` calls.

Persisted-format changes require an ADR, a new version, compatibility fixtures, and rejection tests
for unknown future versions.
