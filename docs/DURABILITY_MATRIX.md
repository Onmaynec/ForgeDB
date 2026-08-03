# Durability Matrix

ForgeDB v0.0.1 has no database writes and makes no data-durability claims.

| Operation | Linux | Windows | Notes |
|---|---|---|---|
| Exclusive `LOCK` lifetime | OS advisory exclusive lock; released when handle/process closes | OS exclusive lock; released when handle/process closes | Other software that ignores locks can still access files. |
| New `IDENTITY` installation | file `sync_all`, then directory sync | file `sync_all`, then directory handle sync | Exact power-loss guarantees remain filesystem and hardware dependent. |
| `rename_atomic` abstraction | delegates to `std::fs::rename` | delegates to `std::fs::rename` | Replacement semantics are not yet used by a persisted protocol. |

The WAL milestone must expand this document with process-crash, OS-crash, and power-loss behavior for
every durability mode.
