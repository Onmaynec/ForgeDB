# ADR 0016 — FQL V1 поверх CLI

- Статус: принято для v0.8.0
- Дата: 2026-11-14

## Контекст

В v0.7.0 доступны ColumnFamilies, MVCC snapshots и optimistic transactions, но для ручного выполнения запросов требовалось вызывать отдельные CLI команды или писать программу на Rust. Введение SQL/JOIN/глобального execution planner потребовало бы существенно большего контракта и противоречило цели минимального безопасного инструмента.

## Решение

FQL V1 — отдельный модуль бинарника forgedb-cli без новых runtime-зависимостей и изменений persisted storage. Каждый statement разбирается в AST целиком до вызовов ColumnFamilies API.

1. Bounded lexer/parser: до 64 KiB текста и 512 токенов, ровно один statement на строку. Лишние tokens, malformed escapes и неверные аргументы отклоняются до mutation.
2. Бинарно-точные quoted literals, \xHH и X'HEX'; результат GET/SCAN форматируется обратимыми quoted values.
3. SCAN выполняется в streaming bounded iterator (100 entries по умолчанию, 10000 максимум). TO bound exclusive.
4. BEGIN создаёт существующую snapshot-isolation Transaction только для одной family; COMMIT всегда с WAL Sync; при EXIT/EOF/script error транзакция откатывается.
5. USE/CREATE/DROP FAMILY и SCAN запрещены внутри BEGIN, пока нет корректного snapshot range merge и межсемейной атомарности.
6. Файл script не является глобальной транзакцией; завершённые statement до ошибки не откатываются, за исключением buffered write set внутри незавершённого BEGIN.
7. One-shot, script и интерактивный stdin проходят через единый parser и executor. Ошибочный script завершает процесс с ненулевым кодом, REPL продолжает.
8. Read-only mode не создаёт базу и не допускает mutation.

## Последствия

FQL — не SQL, не сервер, не отдельный persisted формат. Для совместимого default DB и именованных families остаются WAL V1, SSTable V1, MANIFEST V2, CURRENT V1 и CF_REGISTRY V1. Общие транзакции между families (#35), глобальный FD budget (#31), сетевой API и query optimizer откладываются.

См. docs/FQL.md и docs/releases/V0.8.0.md.
