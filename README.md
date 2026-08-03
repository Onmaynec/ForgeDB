# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust. Проект ставит корректность,
явные гарантии durability, ограниченное использование ресурсов, версионируемые persisted-форматы
и обнаружение повреждений выше преждевременной оптимизации производительности.

## Текущий этап: v0.1.0 — Durable WAL Core

Версия реализует первый рабочий путь записи и восстановления:

- Rust 2024 workspace с минимальной поддерживаемой версией Rust 1.89;
- бинарные ключи и значения в библиотечном API;
- `Database::open`, `put`, `get`, `delete`, `write` и `close`;
- атомарный `WriteBatch`;
- возрастающие `SequenceNumber` с допустимыми пропусками после `DisableWal`;
- WAL V1 с фиксированными заголовками, CRC32C и строгими лимитами до allocation;
- mutable memtable на `BTreeMap` с preflight-проверкой памяти;
- режимы записи `Sync`, `NoSync` и `DisableWal`;
- replay всех полных кадров после повторного открытия;
- безопасное игнорирование и усечение только последнего физически неполного кадра;
- отказ от открытия при checksum mismatch, неизвестной версии, неверном identity или повреждении полного кадра;
- process-level эксклюзивная блокировка каталога базы;
- CLI-команды `put`, `get` и `delete`.

## Пример библиотеки

```rust
use forgedb::{Database, Options, Result, WriteOptions};

fn main() -> Result<()> {
    let database = Database::open("./data", Options::default())?;
    database.put(b"user:42", b"Alice", WriteOptions::sync())?;
    assert_eq!(database.get(b"user:42")?, Some(b"Alice".to_vec()));
    database.delete(b"user:42", WriteOptions::sync())?;
    database.close()?;
    Ok(())
}
```

## CLI

```bash
cargo run -p forgedb-cli -- put ./data key value --sync
cargo run -p forgedb-cli -- get ./data key
cargo run -p forgedb-cli -- delete ./data key --sync
```

CLI v0.1.0 принимает ключи и значения как UTF-8 аргументы командной строки. Библиотечный API
не имеет этого ограничения и работает с произвольными байтами.

## Режимы durability

- `Sync`: успех возвращается только после append кадра, `sync_all` WAL и атомарного применения batch к memtable.
- `NoSync`: batch записывается в WAL и становится видимым без обязательного физического sync перед возвратом.
- `DisableWal`: batch применяется только к memtable и может исчезнуть при process crash.

Точные границы гарантий описаны в [`docs/DURABILITY.md`](docs/DURABILITY.md).

## Структура workspace

```text
crates/forgedb       Публичный API, Database и mutable memtable
crates/forgedb-cli   Командная строка put/get/delete
crates/forgedb-core  Storage-neutral типы, лимиты, internal keys и WriteBatch
crates/forgedb-fs    Файловая абстракция, native lock и database identity
crates/forgedb-wal   Формат WAL V1, writer, replay и CRC32C
```

## Проверка

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

CI выполняет проверки на Ubuntu stable, Windows stable и Rust 1.89.

## Ограничения v0.1.0

В версии ещё нет SSTable, MANIFEST, flush, compaction, snapshots, transactions, column-family API,
WAL rotation и фоновых worker. Memtable имеет жёсткий предел: запись отклоняется до изменения WAL,
если projected memory превышает `Options::memtable_size`.

## Документация

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — границы компонентов;
- [`docs/WRITE_PATH.md`](docs/WRITE_PATH.md) — порядок записи;
- [`docs/RECOVERY.md`](docs/RECOVERY.md) — replay и corruption policy;
- [`docs/formats/WAL_V1.md`](docs/formats/WAL_V1.md) — нормативный бинарный формат;
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md) — проверяемые инварианты;
- [`docs/adr`](docs/adr) — принятые архитектурные решения.

## Лицензия

MIT.
