# CF_REGISTRY V1

Статус: формат имён Column Families для ForgeDB v0.7.0.

## Размещение

Корень прежнего default DB остаётся без изменений. Новые файлы размещаются только в `.forgedb-column-families`:

```text
<database>/
  IDENTITY, CURRENT, MANIFEST-*.log, WAL-*, SST-*
  .forgedb-column-families/
    REGISTRY
    CF-0000000001/
      IDENTITY, CURRENT, MANIFEST-*.log, WAL-*, SST-*
    CF-0000000002/
      ...
```

Числовой ID используется в имени каталога, имя пользователя никогда не подставляется в путь. Допустимы имена из 1–64 ASCII-байт `A-Za-z0-9_-`, кроме зарезервированного `default`; одинаковые имена запрещены. Каждый CF-подкаталог является самостоятельной ForgeDB базой с отдельным `IDENTITY`, WAL, sequence numbers, MANIFEST и LOCK.

## Бинарный layout

Little-endian для всех целых полей. Нет padding; все offsets считаются от начала файла.

| Offset | Размер | Поле |
|---|---:|---|
| 0 | 8 | ASCII magic `FGCFREG1` |
| 8 | 4 | `version = 1` (u32) |
| 12 | 4 | `next_id` (u32), не меньше 1 |
| 16 | 4 | `active_count` (u32), максимум 1024 |
| 20 | variable | Последовательность `active_count` записей |
| EOF − 4 | 4 | CRC32C всего файла до checksum |

Запись:

```text
column_family_id: u32 LE (1 <= id < next_id)
name_length:      u8     (1..=64)
name_bytes:       name_length bytes; ASCII A-Za-z0-9_-
```

Порядок записей — по возрастающему ID. Повторы ID, имён, зарезервированное `default`, пустые имена, trailing bytes после последней записи, невозможные lengths и превышение 128 KiB считаются ошибкой. Unknown future registry version возвращает `UnsupportedFormat`, CRC mismatch — `Corruption`. Ошибка реестра не вызывает автоматической реконструкции по каталогам.

`next_id` монотонно увеличивается. Удалённый ID не возвращается в набор активных и не переиспользуется в будущем.

## Crash-safe публикация

1. При `create`: выбрать ID, не занятый ни реестром, ни существующим физическим каталогом (включая orphan), открыть и закрыть новый CF как обычную базу.
2. Создать/заменить `REGISTRY.tmp` полным новым snapshot реестра.
3. `write_all` и `sync_all` временного файла.
4. `rename_atomic(REGISTRY.tmp, REGISTRY)`.
5. `sync_directory` реестра и корня базы.
6. Только после успеха публикации изменить in-memory registry.

При crash до шага 4 старый реестр authoritative; созданный CF-каталог остаётся orphan, не подключается автоматически и не удаляется. После durable rename authoritative является новый реестр. `drop_family` сперва синхронизирует и закрывает CF, затем публикует snapshot без его ID. Физические файлы drop сохраняются для диагностики.

## Read-only и несовместимость

Read-only открытие реестра не создаёт файлов/каталогов. Отсутствующий `REGISTRY` для старой single-family базы означает только default CF. Если зарегистрированный CF отсутствует, не создаётся пустой заместитель — операция возвращает `Corruption`.

WAL V1, Internal Key V1, SSTable V1, MANIFEST reader V1/V2, writer V2 и CURRENT V1 каждого каталога не изменены. CF registry не означает поддержку межсемейных atomic batches или общего WAL.
