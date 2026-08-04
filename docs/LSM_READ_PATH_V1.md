# Полный LSM read path ForgeDB v0.3

## Статус

Документ описывает read semantics milestone `v0.3.0`. Он не вводит новый persisted-формат: используются WAL V1, SSTable V1, MANIFEST V1 и CURRENT V1 из предыдущих этапов.

## Read snapshot

Каждая операция чтения сначала фиксирует один `snapshot_sequence`:

- `ReadOptions::snapshot = Some(snapshot)` использует sequence указанного snapshot;
- без явного snapshot используется последний опубликованный sequence на момент начала операции;
- пустая база представляется отсутствующим sequence и не имеет видимых записей.

Один read не должен менять snapshot между источниками.

## Порядок point lookup

Источники проверяются строго в следующем порядке:

1. mutable memtable;
2. immutable memtable незавершённого flush;
3. L0 SSTables от большего file number к меньшему;
4. L1 и последующие уровни по возрастанию номера уровня.

Внутри одного источника выбирается первая версия user key, для которой `sequence <= snapshot_sequence`.

Результаты имеют три состояния:

- `Value(bytes)` завершает lookup значением;
- `Tombstone` завершает lookup отсутствием значения;
- `Missing` разрешает перейти к следующему источнику.

Tombstone никогда не разрешает искать более старое значение в нижнем источнике.

## VersionSet

Readers используют immutable `Arc<VersionSet>`. Публикация нового SSTable создаёт новый VersionSet и атомарно заменяет ссылку в состоянии базы. Уже начавшийся reader может продолжить работу со старой версией без удержания writer mutex.

### L0

- диапазоны могут перекрываться;
- file numbers строго возрастают в metadata;
- чтение выполняется newest-first.

### L1+

- файлы отсортированы по `smallest_user_key`;
- соседние диапазоны не пересекаются;
- для одного user key допускается не более одного candidate-файла на уровень.

MANIFEST V1 текущей stacked-реализации сохраняет только L0. Поэтому L1+ уже моделируются и валидируются в памяти, но остаются пустыми до milestone compaction `v0.4.0`.

## Range scan

`Database::scan` создаёт forward user-key iterator.

Алгоритм:

1. фиксирует snapshot и immutable VersionSet;
2. отбирает memtable и SSTable sources, пересекающиеся с range;
3. каждый source предоставляет internal keys в canonical comparator order;
4. binary heap выбирает минимальный internal key;
5. все версии одного user key собираются вместе;
6. версии выше snapshot отбрасываются;
7. первая видимая версия определяет значение или tombstone;
8. tombstones и shadowed versions не попадают в user output.

User output строго отсортирован по байтам user key и не содержит дубликатов.

## Cursor model

Merge iterator работает через object-safe `EntryCursor`:

- mutable и immutable memtables используют `VecEntryCursor` над frozen sorted entries;
- каждый SSTable использует отдельный `SstEntryCursor`;
- range filtering и source-order validation выполняются при продвижении cursor;
- heap хранит только текущий internal key каждого source.

`SstEntryCursor::open` выполняет bounded preflight:

1. сверяет physical size и MANIFEST metadata;
2. читает и проверяет header;
3. читает footer;
4. проверяет filter, index и properties block trailers;
5. декодирует index и проверяет block handles;
6. загружает первый data block.

После исчерпания текущего data block cursor выполняет `seek + read_exact` следующего handle, проверяет trailer/CRC32C, декодирует entries и освобождает предыдущий block.

## Граница памяти

Для выбранного SSTable source одновременно удерживаются:

- metadata и file handle;
- полностью декодированный index;
- entries одного текущего data block;
- небольшое состояние metadata aggregation.

Полное содержимое SSTable не материализуется. Для range с несколькими SSTables resident data memory ограничена одним block на каждый активный source плюс heap и memtable sources. Index пока загружается целиком; block/table cache отсутствуют.

Unit test строит SSTable из нескольких data blocks и подтверждает, что cursor проходит все entries при `blocks_loaded > 1`, а максимальное число resident entries меньше общего числа записей. Integration test выполняет один range scan через три установленных L0 SSTables.

## Corruption policy

До или во время выдачи данных scanner проверяет:

- UUID и file number;
- format versions;
- header/footer CRC32C;
- block handles и отсутствие перекрытий;
- compression/checksum identifiers;
- restart array bounds и ordering;
- internal-key ordering внутри и между blocks;
- tombstone payload;
- entry count, key range и sequence range из MANIFEST при полном достижении конца таблицы.

Unread data blocks не читаются заранее. Поэтому corruption в block, который range/limit не затронул, обнаруживается deep verify или чтением соответствующего диапазона, а не коротким lazy scan.

При нарушении контракта чтение завершается `Error::Corruption`, `Error::UnsupportedFormat` или contextual I/O error. Resynchronization и угадывание структуры не допускаются.
