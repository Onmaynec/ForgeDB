# ADR 0011: Optimistic snapshot isolation в v0.5.0

- Статус: принято для v0.5.0
- Дата: 2026-08-09

## Контекст

К v0.4.0 ForgeDB уже имеет глобальные sequence numbers, MVCC internal keys, зарегистрированные snapshots, snapshot-aware point/range reads, atomic WAL `WriteBatch` и compaction retention относительно oldest active snapshot.

Следующий milestone должен добавить transactions без второго независимого durability protocol и без изменения persisted-форматов.

Основные риски:

1. race между conflict validation и обычным writer;
2. сравнение значений вместо version sequence, пропускающее ABA-сценарий;
3. частичный transaction recovery;
4. удаление compaction-ом версии, всё ещё нужной активной transaction;
5. случайное обещание serializable semantics при реализации только snapshot isolation.

## Решение

ForgeDB v0.5.0 использует optimistic snapshot isolation.

### Snapshot на BEGIN

`Database::begin_transaction()` создаёт обычный зарегистрированный `Snapshot`. Этот snapshot остаётся активным до terminal state transaction.

### Buffered write set

`put` и `delete` добавляются в локальный `WriteBatch`. До commit они не входят в database visible sequence.

Reads проверяют последнюю собственную buffered mutation ключа, затем читают database state на snapshot sequence.

### Read set limits

Read keys проверяются по лимиту `max_key_size` до копирования. Для набора прочитанных уникальных ключей действуют `max_batch_operations` (число ключей) и `max_batch_size` (сумма длины ключей). Read set ограничен по ресурсам, но не участвует в read-write conflict detection.

### Conflict rule

При commit для каждого уникального ключа write set определяется newest committed sequence во всех live источниках. Если он строго больше snapshot sequence transaction, commit отклоняется `Error::Conflict`.

Конфликт определяется по sequence number, а не по текущим value bytes. Поэтому сценарий `A → B → A` после BEGIN всё равно конфликтует с transaction, начатой на первой версии `A`.

### Serialization boundary

Обычные public `put`, `delete`, `write` и transaction validation используют один `write_gate`.

Transaction удерживает gate от начала conflict validation до передачи полного batch в существующий serialized WAL write path. Это исключает public write между успешной validation и commit append.

Flush/compaction не создают новые committed sequence numbers и потому не нарушают правило конфликта; они могут только перемещать уже committed versions между read sources.

### Atomic persistence

После successful validation transaction не получает отдельный log record type. Полный write set передаётся существующему `Database::write` одним `WriteBatch`, который WAL V1 кодирует одним logical frame.

### Durability

- `Sync` — существующий sync WAL contract;
- `NoSync` — существующий no-sync WAL contract;
- `DisableWal` — запрещён для transaction commit.

### Isolation scope

v0.5.0 гарантирует write-write conflict detection. Read set bounded-отслеживается, но не валидируется для full read-write conflicts.

Поэтому snapshot isolation не является serializable и допускает write skew. Predicate/range locks и phantom protection отсутствуют.

## Рассмотренные альтернативы

### Сравнивать value bytes на commit

Отклонено. Не обнаруживает ABA и не выражает MVCC version semantics.

### Создать отдельный transaction WAL format

Отклонено. Существующий atomic `WriteBatch` уже предоставляет нужную persistence unit. Новый формат увеличил бы compatibility и recovery surface без необходимости.

### Валидировать только mutable memtable

Отклонено. Более новая committed version может уже находиться в pending immutable memtable или SSTable после flush/compaction.

### Реализовать serializable isolation сразу

Отклонено для v0.5.0. Для корректной защиты predicates/ranges потребовалась бы отдельная модель конфликтов и существенно более широкая архитектура. Scope milestone явно ограничен snapshot isolation.

### Разрешить `DisableWal`

Отклонено. Transaction API не должен скрыто ослаблять crash durability до состояния, где acknowledged commit может целиком исчезнуть до flush без WAL.

## Последствия

Положительные:

- один write/recovery path для обычных и transactional batches;
- persisted formats не меняются;
- active transactions автоматически участвуют в compaction retention через существующий SnapshotRegistry;
- conflict semantics устойчивы к ABA;
- failure до WAL mutation не публикует часть transaction.

Отрицательные:

- public write gate сериализует writes и validation; это correctness-first baseline;
- SSTable conflict lookup в v0.5.0 может быть дороже будущего indexed решения;
- write skew остаётся допустимой anomaly;
- caller отвечает за retry после `Conflict`.

## Invariants

- I-041: own writes имеют приоритет над snapshot state внутри active transaction.
- I-042: commit отклоняется, если written key имеет committed sequence выше snapshot.
- I-043: conflict validation и последующий WAL append не разделяются другим public writer.
- I-044: successful transaction commit использует один atomic WAL WriteBatch.
- I-045: active transaction удерживает registered snapshot до terminal state.
- I-046: transaction commit не принимает `DisableWal`.

## Persisted-format impact

Отсутствует. WAL остаётся V1, SSTable V1, MANIFEST writer V2, CURRENT V1.
