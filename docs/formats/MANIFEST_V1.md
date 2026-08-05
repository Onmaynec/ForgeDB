# Формат MANIFEST V1

## Назначение

MANIFEST V1 является authoritative append-only журналом структуры LSM ForgeDB. Содержимое каталога само по себе не определяет live-файлы: live WAL и SSTable выбираются только через восстановленное состояние MANIFEST.

## Имя и заголовок

Canonical имя: `MANIFEST-{file_number:06}`. Заголовок занимает 64 байта и содержит magic `FORGEMAN`, версию `1`, длину заголовка, database UUID, номер файла, timestamp, reserved bytes и CRC32C.

Reader проверяет magic, версию, header length, UUID, file number, reserved fields и checksum до replay edits.

## Frame VersionEdit

Каждый edit записывается одним frame с фиксированным заголовком 32 байта:

```text
magic: 4 bytes = FMV1
total_length: u32 LE
crc32c: u32 LE
record_type: u8 = 1
flags: u8 = 0
format_version: u16 LE = 1
field_count: u32 LE
payload_length: u32 LE
reserved: u64 = 0
payload
```

Checksum рассчитывается по байтам frame начиная с `record_type`, включая payload. `magic`, `total_length` и сохранённый CRC не входят в checksum.

## Payload

Payload начинается с `edit_version: u8 = 1` и трёх нулевых reserved bytes. Далее следуют bounded TLV-поля:

```text
tag: u8
reserved: 3 bytes = 0
length: u32 LE
value: length bytes
```

V1 определяет поля comparator, current log number, previous log number, next file number, last sequence, add SSTable, delete SSTable, minimum reader version и feature flags. Неизвестный обязательный tag, duplicate singleton field, oversized length и trailing bytes являются ошибкой.

## Инварианты состояния

- comparator совпадает с identity текущего internal-key comparator;
- `log_number`, `manifest_number` и `next_file_number` не равны нулю;
- `next_file_number` больше всех уже использованных номеров;
- один file number не встречается дважды;
- L0 metadata отсортирована по file number;
- `last_sequence` не уменьшается;
- неизвестные minimum reader version и feature flags отклоняются;
- добавляемый SSTable полностью проверяется до публикации состояния.

## Bootstrap initial MANIFEST

Initial MANIFEST не публикуется раньше WAL, на который он ссылается:

1. создать и синхронизировать header-only initial WAL;
2. создать и синхронизировать `MANIFEST-000001`;
3. записать и синхронизировать `CURRENT.tmp`;
4. атомарно установить `CURRENT`;
5. синхронизировать каталог базы.

Если crash произошёл до появления authoritative `CURRENT`, повторная инициализация может удалить только зарезервированные bootstrap artifacts:

- `MANIFEST-000001`;
- `MANIFEST-000001.tmp`;
- `CURRENT.tmp`.

Header-only initial WAL повторно используется только после проверки UUID, file number и отсутствия complete frame/partial tail. Неизвестные и пользовательские файлы не удаляются.

Существующий `CURRENT` запрещает повторный вызов initial MANIFEST initialization.

## Атомарность edit

Writer сначала preflight-применяет edit к clone текущего состояния. Затем кодирует frame, append-записывает его, выполняет `sync_all()` MANIFEST и только после успешного sync заменяет in-memory state. Ошибка append или sync не публикует новый VersionSet.

## Recovery

Replay идёт строго от начала после header. Разрешён только физически неполный последний frame. Полный frame с неверным CRC, неизвестной версией, нарушением полей или invariant блокирует открытие. Normal recovery не ищет следующий magic.

При read-write открытии доказанно неполный хвост усекается до `valid_bytes`, после чего новый размер синхронизируется. Read-only открытие файл не изменяет.

После того как `CURRENT` установлен, отсутствие current или previous WAL, указанного восстановленным MANIFEST, является corruption. Recovery не создаёт пустой replacement WAL.

## Flush edits

Flush v0.2.0 использует два edits:

1. rotation edit публикует новый current WAL и старый WAL как `previous_log_number`;
2. installation edit добавляет полностью синхронизированный SSTable и очищает `previous_log_number`.

Перед rotation edit старый current WAL синхронизируется. Новое WAL-поколение создаётся до append edit. Если после crash остался unreferenced WAL, повторно использовать разрешено только строго проверенный header-only файл; WAL с frame или partial tail сохраняется и блокирует flush.

Если replay обнаружил `previous_log_number`, previous WAL восстанавливается в отдельную immutable pending memtable, а current WAL — в mutable memtable. Смешивание поколений запрещено.

Старый WAL удаляется только после успешного sync installation edit. SSTable становится live только через `added_files` этого edit.
