# Путь записи v0.1.0

## Цель

Путь записи обеспечивает атомарность `WriteBatch`, последовательное назначение sequence numbers,
явный режим durability и отсутствие видимости частично применённого batch.

## Упорядочивание

Все записи сериализуются одним `Mutex<DatabaseState>`. В v0.1.0 это сознательная архитектура:
один упорядоченный writer path важнее параллелизма до появления flush, MANIFEST и compaction.

## Алгоритм

Для `Database::write(batch, options)` выполняются шаги:

1. Проверить, что база не закрыта и не открыта в read-only режиме.
2. Проверить лимиты ключей, значений, размера batch и числа операций.
3. Отклонить пустой batch.
4. Захватить mutex состояния.
5. Отклонить запись, если предыдущая ошибка WAL перевела writer в fatal state.
6. Зарезервировать непрерывный sequence-диапазон для всех операций batch.
7. Подготовить изменения memtable без изменения её видимого состояния.
8. Проверить projected memory memtable до записи WAL.
9. В зависимости от `WriteOptions`:
   - `Sync`: закодировать один WAL-кадр, выполнить `write_all`, затем `sync_all`;
   - `NoSync`: закодировать один WAL-кадр и выполнить `write_all` без обязательного sync;
   - `DisableWal`: пропустить WAL.
10. Если WAL-операция завершилась ошибкой, не применять prepared batch и перевести writer в fatal state.
11. Одной критической секцией применить весь prepared batch к memtable.
12. Переместить `next_sequence` за конец batch.
13. Вернуть успех.

## Атомарность

Один `WriteBatch` хранится одним WAL-кадром и полностью декодируется до replay. Поэтому recovery видит либо
весь batch, либо ни одной его операции. В runtime batch становится доступным читателям только после полного
применения staged changes к memtable под тем же mutex.

## Ошибки до WAL

Следующие ошибки не изменяют WAL, memtable и sequence state:

- пустой ключ;
- превышение лимита ключа, значения, batch или operation count;
- арифметическое переполнение;
- projected memory выше `Options::memtable_size`;
- закрытая или read-only база;
- ранее зафиксированная fatal WAL error.

## Ошибка append или sync

Если append или обязательный sync не завершился успешно:

- prepared batch не применяется к memtable;
- sequence state не продвигается;
- последующие записи отклоняются как `BackgroundFailure`;
- caller должен закрыть и заново открыть базу для проверки WAL и recovery.

Это консервативное поведение исключает продолжение записи после неясного результата I/O.

## DisableWal

`DisableWal` сохраняет обычную атомарность относительно текущего процесса, но не crash durability.
Sequence numbers всё равно резервируются и продвигаются, поэтому persisted WAL может содержать законные
пропуски. Replay разрешает такие пропуски, но запрещает пересечение или уменьшение sequence.

## Что отсутствует

В v0.1.0 нет:

- group commit;
- нескольких writer threads внутри critical section;
- WAL rotation;
- immutable memtable;
- flush и write stall;
- MANIFEST commit;
- SSTable publication.
