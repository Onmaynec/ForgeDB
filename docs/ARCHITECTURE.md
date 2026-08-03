# ForgeDB Architecture

## v0.0.1 component boundaries

```text
forgedb (public facade and Options)
  ├── forgedb-core (errors, identifiers, internal keys, limits, WriteBatch)
  └── forgedb-fs   (filesystem trait, native filesystem, lock, IDENTITY)
         └── forgedb-core
```

Dependency direction is one-way. `forgedb-core` knows nothing about filesystems or public database
orchestration. `forgedb-fs` contains no storage-engine policy. `forgedb` is the only crate intended
for normal application dependencies.

## Current thread model

No background workers or database state exist in v0.0.1. Native lock guards are `Send + Sync` and
hold an operating-system exclusive lock for their lifetime.

## Current ownership model

- Identifiers and internal keys own their bytes.
- `WriteBatch` owns all queued key/value bytes and mutates only after all limits pass.
- The native lock guard owns the locked file handle.
- Identity installation writes, synchronizes, and closes the file before synchronizing the directory.

## Planned next layer

`v0.1.0` adds the first database orchestration layer: a single ordered writer, WAL V1, mutable
memtable, visibility publication, and replay. No SSTable or compaction code should be added before
that write/recovery contract is complete.
