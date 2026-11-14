# ForgeDB v0.8.0 — FQL & Interactive Shell

Статус: GitHub Pre-release.

Версия 0.8.0 добавляет строгий командный язык FQL V1 и интерактивный shell для локальных баз ForgeDB. Запросы выполняются через существующий ColumnFamilies API, без нового storage engine и изменения on-disk форматов. Доступны готовые CLI для Windows и Linux x86-64.

## FQL V1

- Чтение и запись: GET key, PUT key value, DELETE key.
- Диапазон: SCAN [FROM start] [TO end] [LIMIT number]. FROM включительно, TO исключительно.
- Именованные хранилища: SHOW FAMILIES, CREATE FAMILY name, DROP FAMILY name|id, USE name|id.
- Транзакции выбранной family: BEGIN, COMMIT, ROLLBACK.
- Справка и выход: HELP, EXIT и QUIT.

FQL допускает один statement на строку, при желании с завершающей точкой с запятой. Он **не является SQL**: нет SELECT, JOIN, реляционных таблиц и сетевого сервера.

Quoted текст поддерживает пробелы и escape-последовательности; hex-литералы X'00ff' позволяют работать с бинарными ключами/значениями. Результат GET/SCAN записывается в обратимом quoted-формате с \xHH для непечатных байтов.

## Запуск

Одноразовая команда:

    forgedb fql ./data "PUT key 'Hello FQL'"
    forgedb fql ./data "GET key" --read-only

Интерактивный shell:

    forgedb shell ./data

Файл сценария:

    forgedb shell ./data --file commands.fql

Пример commands.fql:

    CREATE FAMILY users
    USE users
    BEGIN
    PUT user42 'Alice'
    GET user42
    COMMIT
    SCAN FROM user TO userz LIMIT 20
    EXIT

Shell поддерживает перенаправленный stdin; при работе с терминалом показывает название выбранной family.

## Поведение при ошибках

- Входной statement полностью разбирается до выполнения: лишние команды, ошибочный escape, некорректный LIMIT и неверные аргументы не инициируют частичную мутацию.
- Максимум 64 KiB на строку; чтение stdin/file ограничено до разбора. При oversized строке REPL дочитывает её до newline и может корректно обработать следующую команду. Максимум 512 токенов.
- SCAN отдаёт не более 100 строк по умолчанию и не более 10000 с LIMIT; реализован streaming iterator без материализации всего результата.
- Файловые скрипты прекращают работу на первой ошибке с номером строки и ненулевым exit code. REPL после ошибочной команды остаётся активным.
- Незавершённая транзакция при ошибке script, EOF или EXIT откатывается. GET внутри BEGIN видит собственные buffered writes; COMMIT использует существующий optimistic snapshot isolation и WAL Sync.
- SCAN, USE, CREATE/DROP FAMILY запрещены внутри BEGIN до поддержки корректного transactional range merge и cross-family semantics.
- --read-only не создаёт хранилище и запрещает изменения.

**Важное ограничение:** скрипт целиком не атомарен. Записи из уже завершённых отдельных statements остаются durable, если позднее возникла ошибка. Для атомарной группы mutations используйте BEGIN–COMMIT **в одной** family.

## Совместимость

FQL реализован на уровне CLI. Форматы IDENTITY V1, Internal Key V1, WAL V1, SSTable V1, MANIFEST writer V2/reader V1+V2, CURRENT V1 и CF_REGISTRY V1 не изменяются. Миграции существующих баз v0.7.0 не требуется. Все прежние команды CLI сохраняются.

Новых mixed-family atomic WAL batches, глобальных MVCC snapshots или serializable transactions нет (см. Issue #35). Управление количеством открытых SST handles остаётся задачей #31. SQL-синтаксис и серверные подключения не добавлены.

## Проверки

Feature PR #37, head 0d86143159ed35b07eaa43e2a789a6d2acefd9db: CI #43555440756 завершён успешно на Ubuntu stable, Windows stable и Rust 1.89.

Проверены rustfmt, строгий Clippy с -D warnings, workspace tests, doctests, release build и benchmark smoke. Добавлены unit tests parser/lexer и integration CLI tests: quoted/binary data, one-shot query, stdin REPL, file scripts, fail-fast, rollback при EOF/error, read-only, Column Families scope, SCAN limits и oversized-line recovery.

Release workflow повторяет Linux validation gate перед созданием аннотированного Git tag v0.8.0 и GitHub Pre-release.

## Windows / Linux CLI binaries

При выпуске в Assets публикуются два архива x86-64 и отдельные SHA-256 checksums:

- forgedb-v0.8.0-windows-x86_64.zip — forgedb.exe;
- forgedb-v0.8.0-linux-x86_64.tar.gz — forgedb.

Rust toolchain для запуска CLI не нужен. Подробности распаковки/проверки контрольных сумм — docs/QUICKSTART.md.

## Документация

- docs/FQL.md — синтаксис и поведение выполнения;
- docs/adr/0016-fql-cli.md — архитектурное решение;
- docs/releases/V0.8.0.md — технические границы milestone;
- docs/COMPATIBILITY.md, docs/VALIDATION.md, docs/INVARIANTS.md.
