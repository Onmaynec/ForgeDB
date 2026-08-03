# Путь чтения v0.1.0

## Point lookup

`Database::get(key)` выполняет:

1. проверку, что база не закрыта;
2. проверку ключа по `Limits`;
3. захват mutex состояния;
4. поиск ключа в mutable memtable;
5. возврат копии последнего значения или `None`.

В v0.1.0 единственным источником пользовательских данных является mutable memtable, заполненная runtime-записями
и WAL replay. SSTable и merge нескольких источников ещё отсутствуют.

## Tombstone

Delete хранится как запись с `None`. Для публичного point lookup tombstone и никогда не существовавший ключ оба
возвращаются как `None`.

## Последняя операция выигрывает

Если один `WriteBatch` несколько раз изменяет один ключ, preflight staging сохраняет последнюю операцию. Между batch
побеждает запись с большим sequence number.

## Консистентность

Читатель использует тот же mutex, что и writer. Поэтому он не может увидеть частично применённый batch:

- до commit видит старое состояние;
- после commit видит состояние после всех операций batch.

## Read-only открытие

Read-only режим:

- требует существующие `IDENTITY` и WAL;
- получает ту же эксклюзивную process-level блокировку, что и read-write режим;
- выполняет полный replay;
- не создаёт и не усекает файлы;
- отклоняет `put`, `delete` и `write` с ошибкой `ReadOnly`.

Эксклюзивная блокировка read-only режима в v0.1.0 является консервативным решением. Shared-lock и несколько читателей
могут появиться только вместе с доказанным multi-process protocol.

## Ограничения

В v0.1.0 отсутствуют:

- iterator и range scan;
- snapshots;
- column-family lookup;
- SSTable reads;
- block cache;
- bloom filters;
- zero-copy возврат value;
- concurrent lock-free readers.
