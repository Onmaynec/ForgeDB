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

## Текущая граница реализации

В `v0.3.0` scanner bounded-загружает и декодирует выбранный SSTable в память перед merge. Верхняя граница задаётся `max_compaction_bytes` и metadata limits. Настоящий block-streaming iterator и cache появятся в следующих milestones.

## Corruption policy

До выдачи данных scanner проверяет:

- UUID и file number;
- format versions;
- header/footer CRC32C;
- block handles и отсутствие перекрытий;
- compression/checksum identifiers;
- restart array bounds и ordering;
- internal-key ordering внутри и между blocks;
- tombstone payload;
- entry count, key range и sequence range из MANIFEST.

При нарушении контракта чтение завершается `Error::Corruption` или `Error::UnsupportedFormat`. Resynchronization и угадывание структуры не допускаются.
