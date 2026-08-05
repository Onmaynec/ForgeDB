# Формат SSTable V1

## Статус

SSTable V1 — первый версионируемый immutable-формат ForgeDB. Writer версии `0.2.x` создаёт только V1. Reader обязан отклонять неизвестную будущую версию.

## Имена файлов

Установленный файл имеет canonical-имя `SST-{file_number:06}.sst`. Временный файл создаётся как `TEMP/SST-{file_number:06}.tmp`. Номер `0` зарезервирован, повторное использование номера запрещено.

## Заголовок

Заголовок занимает 64 байта:

| Смещение | Размер | Поле |
|---:|---:|---|
| 0 | 8 | magic `FORGESST` |
| 8 | 2 | format version = 1, LE |
| 10 | 2 | header length = 64, LE |
| 12 | 4 | flags = 0 |
| 16 | 16 | database UUID |
| 32 | 8 | file number, LE |
| 40 | 8 | smallest sequence, LE |
| 48 | 8 | largest sequence, LE |
| 56 | 4 | reserved = 0 |
| 60 | 4 | CRC32C байтов `[0..60)` |

UUID и file number должны совпадать с `IDENTITY` и MANIFEST metadata.

## Data blocks

Записи отсортированы по logical internal-key comparator. Используется prefix compression:

```text
shared_prefix_length: canonical varint32
unshared_key_length: canonical varint32
value_length: canonical varint32
unshared_key_bytes
value_bytes
```

В конце payload находится массив restart offsets `u32 LE`, затем `restart_count: u32 LE`. Первый restart обязан быть равен нулю, offsets возрастают и указывают внутрь entry area. Default restart interval равен 16.

Каждый физический block заканчивается trailer размером 8 байт:

```text
compression_id: u8
checksum_type: u8
reserved: u16 = 0
checksum: u32 LE
```

V1 поддерживает только `compression_id = 0` и `checksum_type = 1` (CRC32C Castagnoli). Checksum рассчитывается по сохранённому payload и первым четырём байтам trailer.

## Index block

Index V1 содержит separator internal key и `BlockHandle { offset: u64, length: u64 }` для каждого data block. Handles проверяются checked arithmetic и не могут пересекать footer или выходить за физический размер файла.

## Bloom filter

V1 использует один versioned Bloom filter на весь SSTable. Hash layout и количество hashes детерминированы. Возможны false positives, false negatives запрещены. Повреждённый filter является corruption, а не поводом отключить фильтрацию молча.

## Properties block

Properties — bounded TLV-контракт. Он включает file number, entry/value/tombstone counts, границы internal/user keys, sequence range, raw key/value bytes, число data blocks, размеры index/filter и creation timestamp. Reader сверяет свойства с header и MANIFEST metadata.

## Footer

Footer занимает последние 64 байта:

| Смещение внутри footer | Размер | Поле |
|---:|---:|---|
| 0 | 16 | filter block handle |
| 16 | 16 | index block handle |
| 32 | 16 | properties block handle |
| 48 | 2 | format version = 1 |
| 50 | 2 | flags = 0 |
| 52 | 4 | CRC32C байтов `[0..52)` |
| 56 | 8 | magic `FGSSTEND` |

## Установка

1. Выделить уникальный file number через MANIFEST.
2. Записать временный SSTable целиком.
3. Выполнить file sync.
4. Закрыть handle.
5. Выполнить atomic rename в canonical имя.
6. Синхронизировать каталог базы.
7. Append VersionEdit с `added_files`.
8. Синхронизировать MANIFEST.
9. Только после этого добавить reader в current Version и считать файл live.

Файл после rename, но до MANIFEST installation, является orphan и не включается в состояние автоматически.

## Ограничения V1

- только default column family;
- только compression `none`;
- reader версии `0.2.x` bounded-загружает один SSTable в память;
- только L0 point lookup;
- compaction и range iteration не входят в этот milestone.
