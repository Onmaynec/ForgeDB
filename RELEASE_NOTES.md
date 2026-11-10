# ForgeDB v0.5.0 — MVCC Transactions

Статус: GitHub Pre-release.

В этой версии добавлены транзакции с optimistic snapshot isolation поверх существующего WAL/LSM движка. Для транзакционного commit не используется второй журнал: все изменения проходят обычный `Database::write` одним атомарным `WriteBatch`.

## Изменения

- Публичные `Database::begin_transaction()`, `Transaction` и `TransactionState`.
- Snapshot фиксируется в момент BEGIN и удерживается до commit, rollback, conflict либо failure.
- `get` внутри транзакции сначала читает собственные buffered `put`/`delete`, затем данные на snapshot sequence.
- `rollback` отбрасывает изменения без записи в WAL.
- Commit проверяет write-write conflicts по committed sequence numbers во всех live источниках: mutable/immutable memtables и SSTables L0/L1+.
- Конфликт обнаруживается и при ABA (значение изменили, а затем вернули прежние байты).
- Validation и последующая запись WAL сериализованы одним public write gate.
- `Sync` и `NoSync` сохраняют существующие гарантии durability; `DisableWal` при commit запрещён.
- Read set ограничен числом уникальных ключей и их суммарным размером. Некорректный ключ отклоняется до добавления в read set.
- В CLI появились `forgedb transaction` / `forgedb tx`; конфликт возвращает exit code `9`.

## Пример Rust API

```rust
use forgedb::{Database, Options, Result, WriteOptions};

fn main() -> Result<()> {
    let db = Database::open("./data", Options::default())?;
    let mut tx = db.begin_transaction()?;
    tx.put(b"account:1", b"100")?;
    tx.put(b"account:2", b"200")?;
    tx.commit(WriteOptions::sync())?;
    db.close()
}
```

## Пример CLI

```bash
cargo run -p forgedb-cli -- tx ./data --put account:1 100 --put account:2 200 --sync
cargo run -p forgedb-cli -- tx ./data --get account:1 --put account:1 150 --get account:1 --sync
cargo run -p forgedb-cli -- tx ./data --put temporary value --rollback
```

## Гарантии изоляции

Реализована **snapshot isolation**, а не serializable isolation. Обязательная проверка write-write conflicts предотвращает перезапись ключа, изменённого после BEGIN. Read set ведётся для ограничения ресурсов, но не проверяется на все read-write conflicts.

**Не гарантируются:** защита от write skew, predicate/range locking, phantom protection, nested transactions, savepoints и automatic retry. После `Error::Conflict` повтор бизнес-операции остаётся ответственностью вызывающего кода.

## Совместимость файлов

Persisted-format impact: **нет**.

| Компонент | Reader | Writer |
|---|---|---|
| IDENTITY | V1 | V1 |
| Internal Key | V1 | V1 |
| WAL | V1 | V1 |
| SSTable | V1 | V1 |
| MANIFEST | V1/V2 | V2 |
| CURRENT | V1 | V1 |

Commit записывается одним WAL V1 batch frame; recovery не применяет его частично. Автоматическая миграция MANIFEST V1 → V2 при writable open остаётся прежней.

## Тестирование

Проверены собственные записи, rollback, конфликты одного ключа, ABA, conflict после flush в SSTable, конкурентный commit, snapshot retention при leveled compaction, ограничения read set, повторное открытие и восстановление WAL, а также CLI.

Feature validation: [GitHub Actions run 42780867814](https://github.com/Onmaynec/ForgeDB/actions/runs/42780867814) — Ubuntu stable, Windows stable и Rust 1.89 успешно завершились.

Release workflow повторяет Linux gate перед созданием аннотированного тега и GitHub Pre-release.

## Ограничения

- SSTable conflict lookup использует correctness-first bounded decoder и может быть дорогим; оптимизацию оставляю до benchmark в v0.6.0.
- Compaction остаётся синхронной, а physical obsolete SST не удаляются автоматически без reference-aware reaper.
- Полный FQL parser и интерактивная оболочка не входят в этот выпуск.

Подробности: `docs/TRANSACTIONS.md`, `docs/adr/0011-optimistic-snapshot-isolation.md`, `docs/releases/V0.5.0.md`.
