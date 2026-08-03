# Формат WAL V1

Статус: нормативный для ForgeDB v0.1.0.

Все целые числа кодируются в little-endian. Размеры и счётчики проверяются до выделения памяти.
Reader обязан отклонять неизвестные версии, ненулевые reserved-поля, несовпадающий database identity,
неверный номер файла, переполнение длины и checksum mismatch.

## Имя файла

В v0.1.0 используется один файл:

```text
WAL-000001.log
```

Номер файла в заголовке обязан быть равен `1`.

## Заголовок файла: 64 байта

| Смещение | Размер | Поле | Значение |
|---:|---:|---|---|
| 0 | 8 | magic | ASCII `FORGEWAL` |
| 8 | 2 | format_version | `1` |
| 10 | 2 | header_length | `64` |
| 12 | 4 | flags | `0` |
| 16 | 16 | database_id | байты `DatabaseId` |
| 32 | 8 | file_number | `1` |
| 40 | 8 | created_unix_ms | время создания |
| 48 | 12 | reserved | все байты `0` |
| 60 | 4 | header_crc32c | CRC32C диапазона `[0, 60)` |

CRC использует полином Castagnoli. Повреждённый или физически усечённый заголовок всегда блокирует открытие.

## Кадр WriteBatch

Каждый batch хранится одним логическим кадром. Fragmentation и resynchronization в v0.1.0 отсутствуют.

### Заголовок кадра: 32 байта

| Смещение | Размер | Поле | Значение |
|---:|---:|---|---|
| 0 | 4 | frame_magic | ASCII `FWF1` |
| 4 | 4 | total_frame_length | `32 + payload_length` |
| 8 | 4 | frame_crc32c | CRC32C заголовка `[12, 32)` и payload |
| 12 | 1 | record_type | `1` для WriteBatch |
| 13 | 1 | flags | `0` |
| 14 | 2 | reserved | `0` |
| 16 | 8 | sequence_start | первый sequence batch, не `0` |
| 24 | 4 | operation_count | число операций, больше `0` |
| 28 | 4 | payload_length | размер payload |

`sequence_start + operation_count - 1` обязан помещаться в допустимый диапазон `SequenceNumber`.

## Payload

Payload содержит ровно `operation_count` операций подряд.

### Put

```text
operation_type = 1                 u8
column_family                     varint32
key_length                        varint32
value_length                      varint32
key                               key_length bytes
value                             value_length bytes
```

### Delete

```text
operation_type = 0                 u8
column_family                     varint32
key_length                        varint32
value_length = 0                  varint32
key                               key_length bytes
```

После последней операции не допускаются trailing bytes. `value_length` у Delete обязан быть равен нулю.

## Varint32

Используется unsigned LEB128 не длиннее пяти байт. Пятый байт не может содержать значимые биты выше `0x0f`.
Переполненный, слишком длинный или усечённый varint считается corruption.

## Правила replay

1. Заголовок файла читается и полностью проверяется.
2. Кадры читаются строго последовательно без поиска следующего magic.
3. Полный кадр проверяется по структуре, лимитам и CRC32C до применения операций.
4. Batch применяется к memtable только после успешного декодирования всего кадра.
5. Разрешены возрастающие sequence с пропусками, возникшими из-за `DisableWal`.
6. Пересечение с уже воспроизведённым sequence-диапазоном или откат sequence считается corruption.
7. Только последний физически неполный заголовок или payload может быть отброшен.
8. При read-write открытии неполный хвост усекается до последней проверенной границы и синхронизируется.
9. Checksum mismatch полного кадра, неверная сигнатура или структурная ошибка всегда блокируют открытие.

## Ограничения v0.1.0

- один WAL-файл;
- нет rotation;
- нет fragment records;
- нет compression и encryption;
- нет MANIFEST, поэтому номер WAL фиксирован;
- максимальный payload ограничен `Limits::max_batch_size`;
- максимальное число операций ограничено `Limits::max_batch_operations`.

Любое изменение layout требует нового `format_version` и отдельного ADR.
