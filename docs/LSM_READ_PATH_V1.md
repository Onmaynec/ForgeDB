# Полный LSM read path ForgeDB v0.3

Статус: реализован в stacked draft, compiler/CI result отсутствует.

- Один snapshot sequence на read.
- Source order: mutable → immutable → L0 newest-first → L1+.
- Range merge: binary heap, без duplicate keys, shadowed versions и tombstones.
- Memtables: `VecEntryCursor`.
- SSTables: `SstEntryCursor`, decoded index + один current data block.
- Unread blocks проверяются deep verify или чтением соответствующего range.
- Public `Database` временно является streaming wrapper над storage core; consolidation остаётся release gate.

Tests в source tree покрывают multi-block cursor, resident-memory contract, range через три L0 SSTables, snapshots, model, corruption и deep verify. Их зелёное выполнение не подтверждено из-за empty-step CI jobs.
