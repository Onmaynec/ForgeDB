# Транзакции ForgeDB v0.5.0

## Область гарантий

ForgeDB v0.5.0 реализует **optimistic snapshot isolation** для локальной embedded database. Это не serializable isolation.

Каждая transaction:

1. при `Database::begin_transaction()` фиксирует текущий visible sequence через обычный `Snapshot`;
2. читает committed state не новее этого sequence;
3. поверх snapshot видит собственные buffered `put`/`delete`;
4. до commit не публикует buffered mutations другим readers;
5. при commit валидирует write set под тем же public single-writer gate, который используют обычные `put`, `delete` и `write`;
6. при отсутствии конфликта передаёт весь buffered write set одним `WriteBatch` в существующий WAL/memtable write path.

Persisted WAL, SSTable и MANIFEST форматы v0.5.0 не изменяет.

## Public API

```rust
let mut tx = db.begin_transaction()?;

let before = tx.get(b"balance:42")?;
tx.put(b"balance:42", b"150")?;
tx.delete(b"pending:42")?;

tx.commit(WriteOptions::sync())?;
```

Публичное состояние transaction:

```text
Active
Committed
RolledBack
Conflicted
Failed
```

После перехода из `Active` дальнейшие read/write/commit/rollback операции этой transaction отклоняются.

## Own-write visibility

`Transaction::get(key)` проверяет источники в таком порядке:

1. последняя buffered mutation этого ключа внутри transaction;
2. snapshot database state на sequence, зафиксированном при `BEGIN`.

Поэтому:

```text
BEGIN
GET k        -> old
PUT k new
GET k        -> new
DELETE k
GET k        -> not found
```

При rollback эти buffered mutations не записываются в WAL и не становятся visible.

## Write-write conflict detection

Минимальная конфликтная модель v0.5.0 — write-write validation.

Для каждого уникального ключа в write set commit ищет newest committed sequence во всех live read sources:

- mutable memtable;
- pending immutable memtable;
- L0 SSTables;
- L1+ SSTables.

Если найден sequence строго больше transaction snapshot sequence, весь commit завершается:

```rust
Error::Conflict { keys }
```

и ни одна mutation transaction не применяется.

Сравниваются sequence numbers, а не bytes значения. Поэтому изменение ключа после `BEGIN` считается конфликтом даже если другой writer затем вернул значение к тем же bytes, которые видел snapshot.

Validation и последующий WAL append защищены единым public write gate. Другой public writer не может вклиниться между успешной validation и передачей batch в основной write path.

## Read set

`Transaction::get` проверяет ключ по `max_key_size` до копирования в read set. Для уникальных прочитанных ключей одновременно применяются два ограничения:

- число ключей не превышает `Options::max_batch_operations`;
- сумма длин ключей в байтах не превышает `Options::max_batch_size`.

Повторное чтение уже учтённого ключа не расходует бюджет повторно. Если лимит превышен, метод возвращает `ResourceLimit` и не добавляет ключ в read set. Бюджет считает payload ключей; накладные расходы `BTreeSet` дополнительно ограничиваются количеством записей.

В v0.5.0 read set **не используется для read-write conflict validation**. Обязательной является только write-write validation.

Следовательно, ForgeDB v0.5.0 не предотвращает write skew.

## Isolation anomalies

Snapshot isolation v0.5.0 не гарантирует:

- serializable execution;
- predicate locking;
- range phantom protection;
- read-write conflict detection для всех прочитанных ключей;
- protection от write skew;
- nested transactions;
- savepoints;
- automatic retry конфликтующих transactions.

Caller должен повторить business operation на новой transaction после `Error::Conflict`, если retry безопасен для приложения.

## Durability commit

### `WriteOptions::sync()`

Transaction commit использует тот же atomic `WriteBatch` и WAL protocol, что обычный `Database::write`.

Success означает, что полный transaction batch прошёл WAL append и требуемый WAL sync до публикации в memtable согласно действующему write-path contract. После process crash recovery не должна применять только часть такого batch.

### `WriteOptions::no_sync()`

Atomicity batch сохраняется на уровне normal WAL framing/replay, но acknowledged bytes могут быть потеряны при OS crash или power loss согласно `docs/DURABILITY.md`.

### `WriteOptions::disable_wal()`

Для transaction commit запрещён.

`Transaction::commit(WriteOptions::disable_wal())` возвращает `InvalidArgument` и переводит transaction в `Failed` без публикации buffered mutations.

## Snapshots и compaction

Transaction удерживает обычный зарегистрированный `Snapshot` до commit, rollback, conflict или failure.

Поэтому `SnapshotRegistry::oldest_active_sequence()` учитывает активные transactions. Leveled compaction не имеет права удалить historical version, которая ещё может быть видима такой transaction.

После terminal state snapshot handle освобождается.

## SSTable validation path

Conflict detection в v0.5.0 использует correctness-first lookup: для SSTables, чей key range может содержать ключ и whose `largest_sequence` новее snapshot, ForgeDB читает validated entries существующим bounded SST decoder с checksum verification.

Это намеренно не считается финальной performance-реализацией. Более быстрый indexed latest-sequence lookup можно вводить только после benchmark и при сохранении тех же conflict semantics.

## CLI

v0.5.0 добавляет атомарную transaction-команду:

```bash
forgedb transaction ./db \
  --put user:1 Alice \
  --put user:2 Bob \
  --delete pending:1 \
  --sync
```

Alias:

```bash
forgedb tx ./db --put counter 42 --sync
```

Внутри одной transaction также доступен snapshot-aware read:

```bash
forgedb tx ./db --get counter --put counter 43 --get counter --sync
```

Rollback:

```bash
forgedb tx ./db --put temporary value --rollback
```

`--disable-wal` для transaction CLI отклоняется.

Полный FQL lexer/parser и interactive shell остаются milestone v0.8.0; v0.5.0 не добавляет временный ad-hoc FQL parser, который пришлось бы затем поддерживать как случайный public contract.

## Error semantics

- `Conflict` — write set устарел относительно snapshot;
- `InvalidArgument` — invalid transaction state или запрещённая durability;
- `ResourceLimit` — превышен bounded read set или WriteBatch limit;
- `ReadOnly` — storage open не допускает commit;
- storage/I/O/corruption errors переводят commit в `Failed`.

`Error::Conflict.keys` содержит binary keys и не должен безусловно логироваться как plaintext application data.

CLI использует exit code `9` для `Conflict`.

## Persisted-format impact

Нет.

| Формат | v0.5.0 writer |
|---|---:|
| WAL | V1 |
| SSTable | V1 |
| MANIFEST | V2 |
| CURRENT | V1 |

Transaction commit не создаёт отдельного transaction-log формата: атомарность строится на существующем one-frame-per-`WriteBatch` WAL contract.
