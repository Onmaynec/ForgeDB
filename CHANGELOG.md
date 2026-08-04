# История изменений

До v1.0 публичный API может меняться между minor-версиями. Persisted formats меняются только через новую версию, ADR, compatibility tests и migration policy.

## [0.3.0] — в разработке

### Добавлено

- MVCC memtable и snapshot-aware point reads.
- Immutable `Arc<VersionSet>`, L0 newest-first и L1+ non-overlap validation.
- Forward `Database::scan` через binary heap.
- Object-safe `EntryCursor`, `VecEntryCursor` и block-at-a-time `SstEntryCursor`.
- Инкрементальная range filtering и source-order validation.
- Recovery previous/current WAL в разные memtables.
- Retry-safe flush state machine и fatal MANIFEST writer state.
- `FileInventory`, `RecoveryPlan::inspect`, quick/deep verify.
- CLI `scan`, `inventory`, `inspect recovery-plan`, `verify --deep`.
- Snapshot/model/cursor/corruption/recovery tests и русская документация.

### Изменено

- Workspace version `0.3.0`.
- Memtable сохраняет все internal-key versions.
- Readers используют immutable VersionSet.
- Public `Database` временно использует streaming wrapper над storage core; persisted state не меняется.
- SSTable range source загружает index и один data block вместо полного файла.

### Persisted formats

IDENTITY, Internal Key, WAL, SSTable, MANIFEST и CURRENT остаются V1. MANIFEST V1 хранит L0; L1+ data появится с compaction `v0.4.0`.

### Ограничения

- Stacked draft поверх невыпущенной `v0.2.0`.
- Нет compaction, L1+ data, block/table cache, backward iterator, transactions и column-family API.
- Index SSTable загружается целиком.
- Streaming wrapper должен быть объединён с storage core после доступного compiler/refactor pass.
- CI/compiler result пока отсутствует.

## [0.2.0] — в разработке

### Добавлено

- SSTable V1: checksummed blocks, prefix compression, restart points, Bloom filter, properties и bounded decoder.
- MANIFEST V1, VersionEdit и crash-safe CURRENT.
- Numbered WAL/SST generations, mutable/immutable lifecycle и двухфазный flush.
- `Database::flush()`, L0 point reads и recovery previous/current WAL.
- Orphan SST isolation и удаление obsolete WAL после synced installation edit.
- Integration tests и нормативные документы форматов.

### Ограничения

- Только L0, без compaction/range scan/snapshots.
- Flush синхронный.
- Draft не прошёл release gate.

## [0.1.0] — 2026-08-03

### Добавлено

- Durable WAL V1 с CRC32C и strict bounded replay.
- Mutable memtable, `Database` put/get/delete/write/close и atomic WriteBatch.
- Sync/NoSync/DisableWal, read-only open, CLI и crash/model/corruption tests.

## [0.0.1] — 2026-08-03

### Добавлено

- Rust workspace, core types, limits, Internal Key V1, WriteBatch, filesystem abstraction, lock, IDENTITY V1 и CI foundation.
