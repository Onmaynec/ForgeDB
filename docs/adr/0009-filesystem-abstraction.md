# ADR 0009: Route persisted I/O through a filesystem abstraction

- Status: Accepted
- Date: 2026-08-03

Storage code will depend on the `FileSystem` trait rather than direct global filesystem calls. The
native implementation uses safe standard-library APIs, including operating-system file locking
available since Rust 1.89. Fault injection will be added with the first crash-recovery milestone.

Directory synchronization and rename behavior remain platform-specific and must be documented rather
than generalized into unsupported power-loss claims.
