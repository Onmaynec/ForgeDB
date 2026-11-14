# ForgeDB v1.0.0 — Stable General Availability

ForgeDB v1.0.0 — первый **стабильный выпуск** локального встраиваемого LSM key-value движка на Rust и консольного инструмента для Windows/Linux x86-64. В отличие от Pre-release версий v0.x, для него закреплены поддерживаемые API и форматы данных, обязательная кроссплатформенная проверка, документация перехода и выпуск бинарников только после их реального тестирования.

## Что нового в v1.0.0

### 1. Перенос данных между системами: export и restore

Добавил команды:

```bash
forgedb export ./data ./backup.fdbk
forgedb restore ./backup.fdbk ./restored-data
forgedb doctor ./restored-data --deep
```

Архив `LOGICAL_ARCHIVE V1` содержит логические видимые пары ключ–значение **всех действующих Column Families**: обычной `default` и именованных. Поддерживаются произвольные бинарные ключи/значения, включая NUL, 0xFF и значения больше 128 KiB в пределах настроенных лимитов. Сохраняются имена families; их **числовые ID могут измениться** при восстановлении, потому после импорта используйте `cf list`.

Архив имеет magic `FGDBBK01`, version 1, границы записей и итоговую CRC32C Castagnoli. Проверяются размеры до выделения памяти, порядок ключей, повторяющиеся имена, неизвестная версия, обрыв, checksum и лишние байты. Выходной архив создаётся во временном файле, синхронизируется и публикуется без перезаписи существующего. Restore **не изменяет действующую базу**: записывает во временный каталог, проверяет полный архив, закрывает WAL и публикует результат только в **новый** каталог. При обнаружении ошибок целевой каталог не создаётся.

**Границы:** каждая family экспортируется на **собственном MVCC snapshot**. Единой атомарной точки между независимыми families нет, как и глобальных mixed-family транзакций. Для согласованного multi-family экспорта остановите запись приложения. Это логический перенос, не поблочная резервная копия запущенного WAL/LSM. CRC32C обнаруживает случайные повреждения, но не является шифрованием или криптографической подписью.

Спецификация: [LOGICAL_ARCHIVE V1](docs/formats/LOGICAL_ARCHIVE_V1.md).

### 2. Безопасная диагностика базы

```bash
forgedb version
forgedb doctor ./data
forgedb doctor ./data --deep
```

`doctor` выполняет проверку recovery plan и quick/deep verification в **read-only** режиме. При повреждениях и blockers возвращает код ошибки; он **не** удаляет файлы, не исправляет их автоматически и не запускает миграцию. Существующие CLI-команды `get` и `scan` тоже переведены на `read_only=true` и `create_if_missing=false`: опечатка в имени каталога больше не создаёт неожиданную новую базу.

### 3. Быстрее подготовка диапазонных запросов

При `SCAN` на уровнях L1+ с непересекающимися key ranges применяются `partition_point` и верхний cutoff, выбирающие кандидатов за **O(log N + K)** вместо полного просмотра всех SST на уровне. Для L0, где диапазоны могут пересекаться, прежний newest-first проход остаётся. Точные включённые/исключённые границы, MVCC snapshots, tombstones, проверка целостности и lifetime файлов сохранены.

Это улучшение сложности планирования, **не гарантия универсального роста throughput** без парного performance benchmark на одной среде.

### 4. Реальный stable release pipeline

Сборка v1.0.0 проходит Ubuntu/Windows stable и минимальную версию Rust **1.89**: rustfmt, строгий Clippy `-D warnings`, workspace/unit/integration/crash/recovery tests, doctests, `cargo doc`, release build и benchmark. Дополнительно release workflow строит CLI из **аннотированного immutable Git tag**, запускает на обеих ОС реальную последовательность `version → put → doctor --deep → export → restore → get`, проверяет содержимое и SHA-256 архивов.

GitHub Release сначала создаётся как **Draft**; публикация в статусе **Stable** выполняется только после успешной сборки **обеих** платформ и проверки четырёх вложений:

- `forgedb-v1.0.0-windows-x86_64.zip` и `.zip.sha256`;
- `forgedb-v1.0.0-linux-x86_64.tar.gz` и `.tar.gz.sha256`.

## Реально проверенное покрытие до релиза

Функциональный [PR #49](https://github.com/Onmaynec/ForgeDB/pull/49) успешно прошёл [CI #43565530780](https://github.com/Onmaynec/ForgeDB/actions/runs/43565530780) на Ubuntu stable, Windows stable и Rust 1.89, точный исходный commit `3e49c667bf61f76cd2c483f962555d6cf91c972a`. Новые регрессии проверяют:

- `stable_ga_cli`: deep doctor, отсутствие новых файлов при чтении несуществующей базы, binary key и 128-KiB value, три families, tombstones, restore после закрытия;
- отказ от overwrite, повреждения CRC32C, усечения архива, неизвестной будущей версии и публикации частично восстановленного target;
- `stable_range`: Included/Excluded/Unbounded bounds, snapshots, tombstones, compaction и reopen;
- CRC32C known vector `123456789 → 0xE3069283`;
- все предыдущие suites: WAL, SST, MANIFEST, crash recovery, transactions, FQL, compaction, Windows cleanup, Column Families и benchmark smoke.

## Совместимость и установка

Persisted `IDENTITY V1`, `CURRENT V1`, `WAL V1`, `SSTable V1`, `MANIFEST` reader V1–V2 / writer V2 и `CF_REGISTRY V1` **не изменены** относительно v0.9.0. Миграция дисковых файлов базы не требуется. Отдельный формат `LOGICAL_ARCHIVE V1` не используется движком для обычной записи.

Готовые portable CLI для Windows/Linux не требуют установленного Rust. Для подключения **библиотеки** из исходников используется Git tag:

```toml
[dependencies]
forgedb = { git = "https://github.com/Onmaynec/ForgeDB.git", tag = "v1.0.0" }
```

**Важно:** имя `forgedb` в crates.io уже занято **другим** проектом. Эта версия не заявляется опубликованной в crates.io. Руководство: [QUICKSTART](docs/QUICKSTART.md) и [переход с v0.9](docs/MIGRATION_TO_V1.md).

## Известные ограничения стабильной версии

- `WriteBatch`/transactions атомарны **внутри одной Column Family**; нет mixed-family WAL/atomic commit или общего snapshot — [Issue #35](https://github.com/Onmaynec/ForgeDB/issues/35).
- `Options::table_cache_capacity` управляет resident SST handles **одной** базы, не всеми OS FD процесса — [Issue #31](https://github.com/Onmaynec/ForgeDB/issues/31).
- Не решено название публикации в crates.io — [Issue #27](https://github.com/Onmaynec/ForgeDB/issues/27).
- Нет сетевого SQL-сервера, HA, репликации, encryption-at-rest, authentication, tamper-proof archive, secure deletion или гарантированных SLA.
- CRC32C предназначена для обнаружения повреждения, не для защиты от активного атакующего. Стабильный релиз не заменяет тесты восстановления и резервные копии приложения.

Подробные гарантии: [GA_READINESS](docs/GA_READINESS.md), [спецификация архива](docs/formats/LOGICAL_ARCHIVE_V1.md), [совместимость](docs/COMPATIBILITY.md), [ADR 0020](docs/adr/0020-stable-ga-portable-archive.md), [политика релизов](docs/RELEASE_POLICY.md).