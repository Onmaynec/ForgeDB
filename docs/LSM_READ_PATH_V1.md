# Полный LSM read path ForgeDB v0.3

Статус: реализован в stacked draft, compiler/CI result отсутствует.

## Snapshot и source order

Операция фиксирует один sequence. Point lookup проверяет mutable, immutable, L0 newest-first и L1+. `Value` или `Tombstone` завершают поиск; только `Missing` разрешает перейти дальше.

## Range merge

`Database::scan` использует binary heap и object-safe cursors. Memtables представлены `VecEntryCursor`; SSTables — `SstEntryCursor`. Output отсортирован, не содержит duplicate keys, shadowed versions и tombstones.

## SSTable cursor

При open проверяются physical size, header, footer, service block trailers, decoded index и handles. Data blocks читаются через `seek + read_exact` по одному. Resident memory: frozen memtables, decoded index, один data block на активный SSTable source и heap.

Unread blocks не проверяются lazy scan заранее; их corruption обнаруживается deep verify или чтением соответствующего диапазона.

## VersionSet

Readers удерживают immutable `Arc<VersionSet>`. L0 допускает overlap и читается newest-first. L1+ валидируется как sorted/non-overlapping, но остаётся пустым до compaction `v0.4.0`.

## Integration boundary

Public `Database` временно является streaming wrapper над storage core. Wrapper проксирует storage API и заменяет только scan. До релиза типы должны быть объединены после compiler/refactor pass без изменения API или persisted state.

## Tests в source tree

- multi-block cursor и resident-memory contract;
- range через три L0 SSTables;
- snapshot/model/corruption/deep-verify tests.

Их выполнение не считается подтверждённым до восстановления CI.
