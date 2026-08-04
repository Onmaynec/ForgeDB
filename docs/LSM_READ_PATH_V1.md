# Полный LSM read path ForgeDB v0.3

## Статус

Документ описывает реализованный в stacked draft read path. Persisted formats не меняются: WAL, SSTable, MANIFEST и CURRENT остаются V1. Compiler/test result пока отсутствует, потому что GitHub-hosted jobs завершаются до checkout.

## Snapshot

Каждая операция фиксирует один `snapshot_sequence`. Без explicit snapshot используется последний опубликованный sequence на момент начала операции. Версии выше snapshot невидимы.

## Point lookup

Порядок источников:

1. mutable memtable;
2. immutable memtable незавершённого flush;
3. L0 newest-first;
4. L1+ по возрастанию уровня.

`Value` и `Tombstone` завершают поиск. Только `Missing` разрешает перейти дальше.

## VersionSet

Readers удерживают immutable `Arc<VersionSet>`. L0 может перекрываться и читается newest-first. L1+ валидируется как sorted/non-overlapping, но физически остаётся пустым до compaction `v0.4.0`.

## Range scan

`Database::scan` объединяет sorted sources через binary heap. Для каждого user key собираются все версии, отбрасываются sequence выше snapshot, затем первая видимая версия определяет value или tombstone. Output строго отсортирован и не содержит duplicate keys.

## Cursor model

- memtables: `VecEntryCursor`;
- SSTables: `SstEntryCursor`;
- heap: один текущий internal key на source;
- filtering и source-order validation выполняются инкрементально.

`SstEntryCursor` проверяет physical size, header, footer, filter/index/properties trailers и block handles. Index декодируется один раз. Data blocks читаются через `seek + read_exact` по одному; предыдущий block освобождается перед публикацией следующего.

Resident memory включает frozen memtables, decoded index и один data block на активный SSTable source, плюс heap. Полный SSTable не materialize-ится.

## Public API integration

Public alias `Database` временно указывает на streaming wrapper над storage core. Wrapper проксирует write/get/snapshot/flush/inventory/verify/close и заменяет только range scan. После доступного compiler/refactor pass оба типа должны быть объединены без изменения API или persisted state.

## Corruption policy

Scanner проверяет UUID, file number, versions, CRC32C, block handles, restart arrays, internal-key ordering, tombstone payload и metadata consistency. Unread blocks не загружаются заранее: corruption вне затронутого range/limit обнаруживается deep verify или последующим чтением соответствующего block.

Resynchronization и угадывание структуры запрещены.

## Acceptance tests в source tree

- multi-block cursor;
- resident entries меньше full table;
- range через три L0 SSTables;
- snapshot model;
- corruption и deep verify.

Их зелёное выполнение ещё не подтверждено CI.
