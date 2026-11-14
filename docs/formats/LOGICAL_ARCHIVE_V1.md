# LOGICAL_ARCHIVE V1 — переносимый экспорт ForgeDB

Статус: публичный формат с релиза **v1.0.0**. Область: CLI `forgedb export` / `forgedb restore`. Это **НЕ** WAL, SSTable, MANIFEST, файл физического checkpoint или сетевой протокол.

## Назначение и границы

Экспортирует **только логические видимые пары key/value** из действующих `ColumnFamilies` (включая `default`) в переносимый двоичный архив `.fdbk`. Все значения бинарно точные, в том числе NUL и UTF-8-некорректные байты. Удалённые tombstone-записи не переносятся как отдельные события: после restore соответствующие ключи отсутствуют. Архив не переносит историю MVCC, исходные WAL/MANIFEST file numbers, sequence numbers, физический layout SST или идентификаторы открытых database handles.

Каждая family получает собственный независимый MVCC snapshot до начала последовательного экспорта. **Общей атомарной точки времени между families нет**, поскольку до завершения Issue #35 WAL/MVCC независимы. Для multi-family приложений согласованный checkpoint обеспечивается внешней остановкой записей. Snapshot pinning предотвращает смешивание старых/новых значений **в пределах одной family**.

При восстановлении сохраняются имена действующих families и данные, но **числовые ColumnFamilyId могут измениться** (если исходный реестр имел пропущенные ID); нужно повторно находить ID по имени через `cf list`. Удалённые/незарегистрированные family каталоги не импортируются. Пользовательские файлы, unknown artifacts и сведения о физическом расположении базы не являются содержимым архива.

## Порядок байтов и поля

Все многобайтовые целые числа — unsigned **little endian**. Записи образуют сплошной поток без padding. Физическое смещение рассчитывается последовательно:

| Запись | Тип/длина | Содержание |
|---|---|---|
| magic | 8 bytes | ASCII `FGDBBK01` |
| format_version | u32 | Ровно `1` |
| family_count | u32 | 1–1025, включая default |
| family_start | u8 | Ровно `0x01` |
| name_bytes | u16 | Длина ASCII name, не более 64 |
| name | byte[name_bytes] | UTF-8 ASCII; первая family строго `default` |
| entry | u8 | `0x02` для каждой видимой пары |
| key_length | u32 | Длина бинарного key |
| value_length | u32 | Длина бинарного value |
| key | byte[key_length] | Бинарный key без терминатора |
| value | byte[value_length] | Бинарное значение |
| family_end | u8 | `0x03`, повторить блок family_start…family_end ровно family_count раз |
| archive_end | u8 | `0xFF`, после последней family |
| crc32c | u32 | CRC32C Castagnoli от **всех** предыдущих байтов, начиная с magic и включая archive_end |
| EOF | 0 bytes | Лишние байты после checksum — ошибка |

Внутри каждой family keys идут **строго по возрастанию в лексикографическом порядке байтов**, без дубликатов. Повтор имени family запрещён. Неизвестная версия/marker, неправильная first-family, нарушение порядка, недопустимые размеры, неожиданный EOF, лишние байты и несовпадение CRC — отказ. Parser ограничивает длины текущими `Options::max_key_size` и `Options::max_value_size` **до выделения памяти**.

CRC32C: reflected polynomial `0x82F63B78`, initial `0xFFFFFFFF`, final XOR `0xFFFFFFFF`; тестовый вектор `123456789` даёт `0xE3069283`. CRC предназначен для обнаружения случайного повреждения, **не** для подлинности, защиты от умышленной подмены, шифрования либо защиты от rollback.

## Команды и восстановление

```bash
forgedb export ./data ./data-backup.fdbk
forgedb restore ./data-backup.fdbk ./data-restored
forgedb doctor ./data-restored --deep
forgedb cf list ./data-restored
```

Экспорт открывает исходную группу в `read_only` и создаёт файл архива через временный файл **в каталоге назначения**, синхронизирует его и публикует с помощью `hard_link` (операция создания ссылки отказывает, если файл назначения уже существует). Требуется поддержка hard links в файловой системе. При ошибке временный файл удаляется; существующий архив никогда не перезаписывается.

Restore **отказывается от существующего каталога назначения**. Вначале проверяет magic/version и размер, затем создаёт временный каталог рядом с новым местом. Все данные и families применяются в staging-каталоге с проверкой checksums и чтением до EOF; `ColumnFamilies::close` синхронизирует файлы до попытки rename staging → destination. При ошибке checksum, чтения, записи или близкого к лимиту ресурса staged каталог удаляется, а новый destination не публикуется. Предполагается trusted filesystem и отсутствие конкурирующей внешней операции, создающей destination в момент rename. **Нет** гарантии OS-level fail-atomic rename/parent-dir durability для всех файловых систем и сценариев power loss: проверьте целостность после восстановления.

Restore выполняется **только в новый каталог**, не накладывает изменения на существующую базу. После успешного восстановления рекомендуется `forgedb doctor … --deep`. Это логический перенос, не механизм непрерывной репликации, point-in-time online physical backup или disaster recovery с SLA.

## Тестовые контракты v1.0.0

- Export/restore default и named families, включая name из ASCII digits, бинарные ключи и values размером 128 KiB, tombstones, проверка после reopen.
- Проверка CRC32C known vector.
- Rejection: повторный restore в существующую директорию, export поверх существующего архива, повреждённая CRC, усечённый файл, future version.
- Отсутствие видимого частично восстановленного target и временных `.forgedb-restore-*` после обычной ошибки.
- Гарантии WAL/SST/MANIFEST/CF_REGISTRY форматов не меняются: этот файл имеет отдельную версию и не является storage engine upgrade.
