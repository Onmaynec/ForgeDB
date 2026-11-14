# ForgeDB Query Language — FQL V1

## Назначение

FQL V1 — командный язык над локальным key-value storage ForgeDB. Он не является SQL: нет JOIN, SELECT, таблиц и сервера. Исполнитель использует существующий ColumnFamilies API, без новых persisted-форматов или внешних зависимостей.

## Запуск

    forgedb fql ./data "PUT account 'Alice Smith'"
    forgedb fql ./data "GET account" --read-only
    forgedb shell ./data
    forgedb shell ./data --file queries.fql
    forgedb shell ./data --read-only --file report.fql

Команда fql выполняет ровно один statement из одного аргумента CLI; синтаксис проверяется до открытия базы, поэтому неправильная команда не создаёт новую базу. shell без --file открывает REPL (или читает перенаправленный stdin), с --file читает UTF-8 script построчно. --read-only требует существующий каталог и запрещает запись или создание family.

## Команды FQL V1

| Команда | Назначение |
|---|---|
| SHOW FAMILIES | Список зарегистрированных ID и имён |
| CREATE FAMILY users | Создание именованной family; печатает новый ID |
| DROP FAMILY users | Логическое удаление, физические файлы остаются |
| USE users / USE 1 / USE default | Переключить family на время текущего shell |
| PUT key value | Синхронная запись; внутри BEGIN — buffered mutation |
| GET key | Значение или NULL; внутри BEGIN видит собственные writes |
| DELETE key | Записать tombstone |
| SCAN FROM a TO z LIMIT 10 | Включительно FROM, исключительно TO |
| BEGIN / COMMIT / ROLLBACK | Snapshot-isolation транзакция одной family |
| HELP / EXIT / QUIT | Справка и завершение shell |

Операнды FROM, TO, LIMIT у SCAN необязательные и допускаются в любом порядке, но не могут повторяться. LIMIT по умолчанию 100, максимум 10000, ноль возвращает пустой результат. SCAN использует bounded streaming iterator.

В каждой строке ровно один statement, финальная точка с запятой допускается. Разделять несколько команд точкой с запятой внутри одной строки запрещено. Пустые строки и строки, начинающиеся после пробелов с # или --, являются комментариями. Keywords регистронезависимы, имена families — точные.

## Бинарные литералы

Для пробелов используйте одинарные кавычки:

    PUT account 'Alice Smith'
    PUT X'00ff' X'deadbeef'
    GET X'00ff'
    PUT 'key with spaces' 'line\nnext'

В quoted literal доступны escapes: обратная косая черта, одинарная кавычка, \n, \r, \t и \xHH. Hex literal X'...' содержит чётное число hexadecimal digits. Bare token — UTF-8 bytes без пробелов. GET выводит значение в обратимом quoted формате, NULL — отсутствующий ключ; SCAN выводит quoted key, TAB и quoted value. Непечатные bytes показываются как \xHH.

## Транзакции и безопасность

    USE users
    BEGIN
    PUT user42 'Alice'
    GET user42
    COMMIT

BEGIN привязан к активной family. COMMIT применяет проверку write-write conflicts и атомарно фиксирует buffered writes через WAL Sync, ROLLBACK отменяет их. Внутри BEGIN запрещены USE, CREATE/DROP FAMILY и SCAN (пока отсутствует корректный transactional range merge с own writes). Межсемейных атомарных транзакций и глобальных snapshots нет.

При EOF, EXIT, QUIT или ошибке script незакоммиченная транзакция откатывается. В интерактивном shell ошибка одной команды не прекращает сеанс; для rollback активной транзакции нужно явно вызвать ROLLBACK или выйти.

**Скрипт целиком НЕ атомарен**. Строки до первой ошибочной команды могут быть уже durable. Для группы операций одной family используйте BEGIN...COMMIT. После ошибки файл прекращает выполнение, последующие строки не исполняются.

## Лимиты и обратная совместимость

Размер одного statement не больше 64 KiB, не больше 512 токенов, SCAN max 10000 строк. Синтаксическая ошибка, неверная arity, невалидный hex/escape, повторные параметры, неверные bounds проверяются до side effects конкретного statement. Размеры ключа и значения дополнительно проверяет core ForgeDB.

FQL меняет только CLI. WAL V1, SSTable V1, MANIFEST V2, CURRENT V1, CF_REGISTRY V1 и IDENTITY V1 без изменений. Старые CLI команды продолжают работать. Текущие ограничения Column Families, RAM/FD и recovery не расширяются.
