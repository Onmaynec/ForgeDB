# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust. Проект ставит корректность,
явные гарантии durability, ограниченное использование ресурсов, версионируемые persisted-форматы
и обнаружение повреждений выше преждевременной оптимизации производительности.

## Текущая версия: v0.2.0 — SSTable and Flush

Статус: **GitHub Pre-release**.

Версия реализует первый persisted LSM-слой:

- Rust 2024 workspace с минимальной поддерживаемой версией Rust 1.89;
- бинарные ключи и значения в библиотечном API;
- `Database::open`, `put`, `get`, `delete`, `write`, `flush` и `close`;
- атомарный `WriteBatch`;
- WAL V1 с numbered generations, CRC32C и строгим bounded replay;
- mutable и immutable memtable;
- SSTable V1 с checksummed data/index/filter/properties blocks;
- prefix compression, restart points и versioned Bloom filter;
- MANIFEST V1, `VersionEdit` и crash-safe `CURRENT`;
- point reads `mutable → immutable → L0 newest-first`;
- generation-safe recovery current/previous WAL;
- WAL-first bootstrap новой базы;
- retry-safe flush после header-only orphan WAL/SST;
- obsolete WAL cleanup только после durable MANIFEST installation;
- fail-safe открытие при checksum mismatch, неизвестной версии или отсутствующем live-файле;
- process-level эксклюзивная блокировка каталога базы.

## Пример библиотеки

```rust
use forgedb::{Database, Options, Result, WriteOptions};

fn main() -> Result<()> {
    let database = Database::open("./data", Options::default())?;
    database.put(b"user:42", b"Alice", WriteOptions::sync())?;
    database.flush()?;
    assert_eq!(database.get(b"user:42")?, Some(b"Alice".to_vec()));
    database.delete(b"user:42", WriteOptions::sync())?;
    database.close()?;
    Ok(())
}
```

`flush()` синхронно переводит immutable memtable в проверенный L0 SSTable. Старый WAL удаляется только после успешной durable installation записи в MANIFEST.

## CLI

```bash
cargo run -p forgedb-cli -- put ./data key value --sync
cargo run -p forgedb-cli -- get ./data key
cargo run -p forgedb-cli -- delete ./data key --sync
```

CLI принимает ключи и значения как UTF-8 аргументы командной строки. Библиотечный API работает с произвольными байтами.

## Режимы durability

- `Sync`: успех возвращается после append frame, `sync_all` WAL и атомарного применения batch к memtable.
- `NoSync`: batch записывается в WAL и становится видимым без обязательного физического sync перед возвратом.
- `DisableWal`: batch применяется только к memtable и может исчезнуть при process crash.

Перед WAL rotation current generation синхронизируется независимо от режима отдельных записей. Точные границы гарантий описаны в [`docs/DURABILITY.md`](docs/DURABILITY.md) и [`docs/CRASH_RECOVERY_MATRIX_V0.2.md`](docs/CRASH_RECOVERY_MATRIX_V0.2.md).

## Crash-safety v0.2.0

- initial WAL синхронизируется до публикации `CURRENT`;
- existing `CURRENT` запрещает повторную initial initialization;
- MANIFEST-owned WAL не пересоздаётся пустым при отсутствии;
- previous/current WAL восстанавливаются в разные memtable generations;
- header-only orphan WAL повторно используется только после строгой проверки UUID, file number и frame/tail state;
- non-empty или partial orphan WAL не удаляется и блокирует flush;
- SSTable становится live только через durable MANIFEST installation edit;
- obsolete previous WAL удаляется только после установки SSTable;
- normal open не удаляет произвольные unknown/user files.

## Структура workspace

```text
crates/forgedb           Публичный API, Database и memtable lifecycle
crates/forgedb-cli       Командная строка put/get/delete
crates/forgedb-core      Storage-neutral типы, лимиты, internal keys и WriteBatch
crates/forgedb-fs        Файловая абстракция, native lock и database identity
crates/forgedb-wal       WAL V1, numbered generations, writer и strict replay
crates/forgedb-sstable   SSTable V1, Bloom filter, blocks и TableBuildRequest
crates/forgedb-manifest  MANIFEST V1, VersionEdit, CURRENT и authoritative L0 metadata
```

## Проверка

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked --workspace --doc
cargo build --locked --workspace --release
```

Release gate проходит без lint-исключений на Ubuntu stable, Windows stable и Rust 1.89.

## Ограничения v0.2.0

- только Level 0;
- нет leveled compaction;
- нет range iterator, snapshots и transactions;
- flush синхронный, без background worker и write stalls;
- SSTable reader bounded-загружает файл целиком;
- normal open не удаляет произвольные orphan/unknown files;
- CLI inventory/verify появится в следующем этапе.

## Документация

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — границы компонентов;
- [`docs/WRITE_PATH.md`](docs/WRITE_PATH.md) — порядок записи;
- [`docs/RECOVERY.md`](docs/RECOVERY.md) — replay и corruption policy;
- [`docs/CRASH_RECOVERY_MATRIX_V0.2.md`](docs/CRASH_RECOVERY_MATRIX_V0.2.md) — точки падения и recovery contract;
- [`docs/formats/WAL_V1.md`](docs/formats/WAL_V1.md) — WAL V1;
- [`docs/formats/SSTABLE_V1.md`](docs/formats/SSTABLE_V1.md) — SSTable V1;
- [`docs/formats/MANIFEST_V1.md`](docs/formats/MANIFEST_V1.md) — MANIFEST V1;
- [`docs/formats/CURRENT_V1.md`](docs/formats/CURRENT_V1.md) — CURRENT V1;
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md) — проверяемые инварианты;
- [`docs/releases/V0.2.0.md`](docs/releases/V0.2.0.md) — release notes;
- [`docs/adr`](docs/adr) — принятые архитектурные решения.

## Лицензия

MIT.
