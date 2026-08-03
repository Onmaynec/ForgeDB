# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust. Проект ставит корректность,
явные гарантии durability, ограниченное использование ресурсов, версионируемые persisted-форматы
и обнаружение повреждений выше преждевременной оптимизации производительности.

## Текущий этап: v0.3.0 — Full LSM Read Path

`v0.3.0` разрабатывается как stacked draft поверх ещё не выпущенной `v0.2.0`. Ветка и PR не должны
считаться релизом до успешной Linux/Windows/Rust 1.89 матрицы.

В текущем draft реализованы:

- Rust 2024 workspace с MSRV Rust 1.89;
- бинарные ключи и значения;
- WAL V1, numbered WAL generations и strict replay;
- SSTable V1 с CRC32C, index, Bloom filter, properties и restart points;
- MANIFEST V1 и crash-safe CURRENT;
- mutable/immutable MVCC memtables;
- двухфазный flush в L0;
- immutable `Arc<VersionSet>`;
- L0 newest-first lookup;
- модель L1+ со строгим non-overlap invariant;
- cheap snapshot handles;
- snapshot-aware point reads;
- forward range scan и binary-heap merge iterator;
- file inventory без destructive cleanup;
- quick и deep verification;
- process-level эксклюзивная блокировка каталога базы.

## Пример библиотеки

```rust
use forgedb::{Database, Options, ReadOptions, Result, WriteOptions};

fn main() -> Result<()> {
    let database = Database::open("./data", Options::default())?;

    database.put(b"user:42", b"Alice", WriteOptions::sync())?;
    let snapshot = database.snapshot()?;
    database.put(b"user:42", b"Alice v2", WriteOptions::sync())?;
    database.flush()?;

    assert_eq!(
        database.get_with_options(
            b"user:42",
            ReadOptions::from_snapshot(&snapshot),
        )?,
        Some(b"Alice".to_vec()),
    );

    for row in database.scan(b"user:".to_vec()..b"user;".to_vec(), ReadOptions::default())? {
        let (key, value) = row?;
        println!("{} = {}", String::from_utf8_lossy(&key), String::from_utf8_lossy(&value));
    }

    database.close()?;
    Ok(())
}
```

## CLI

```bash
cargo run -p forgedb-cli -- put ./data key value --sync
cargo run -p forgedb-cli -- get ./data key
cargo run -p forgedb-cli -- delete ./data key --sync
cargo run -p forgedb-cli -- scan ./data [start] [end-exclusive]
cargo run -p forgedb-cli -- inventory ./data
cargo run -p forgedb-cli -- verify ./data --deep
```

CLI принимает ключи и границы как UTF-8 аргументы. Библиотечный API работает с произвольными байтами.
Вывод `scan` использует raw key, TAB и raw value, поэтому формат предназначен для диагностики, а не для
надёжного обмена произвольными бинарными записями.

## Порядок чтения

Point lookup использует один зафиксированный sequence и проверяет:

1. mutable memtable;
2. immutable memtable незавершённого flush;
3. L0 от новых file numbers к старым;
4. L1+ по возрастанию уровня.

`Value` и `Tombstone` завершают поиск. Только `Missing` разрешает перейти к следующему источнику.

Range scan объединяет sorted MVCC sources через binary heap, скрывает версии выше snapshot,
shadowed versions и tombstones.

## Durability

- `Sync`: WAL frame синхронизируется до публикации batch в mutable memtable.
- `NoSync`: WAL frame записывается без обязательного физического sync перед возвратом.
- `DisableWal`: batch существует только в памяти до flush и может исчезнуть при process crash.

Flush сначала публикует WAL rotation в MANIFEST, затем строит и синхронизирует SSTable, после чего
отдельным VersionEdit устанавливает таблицу и только потом делает старый WAL obsolete.

## Структура workspace

```text
crates/forgedb           Public API, MVCC memtables, VersionSet, snapshots, iterators, verify
crates/forgedb-cli       put/get/delete/scan/inventory/verify
crates/forgedb-core      Типы, лимиты, internal keys и WriteBatch
crates/forgedb-fs        FileSystem abstraction, native lock и IDENTITY
crates/forgedb-wal       WAL V1, generations, writer и replay
crates/forgedb-sstable   SSTable V1, block codecs, Bloom filter и reader
crates/forgedb-manifest  MANIFEST V1, VersionEdit и CURRENT
```

## Проверка

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

Release gate требует Ubuntu stable, Windows stable и Rust 1.89. Текущий stacked draft ещё не имеет
подтверждённого зелёного результата: GitHub-hosted jobs завершаются до checkout.

## Ограничения v0.3.0

- `v0.2.0` и `v0.3.0` ещё не выпущены.
- Нет compaction и физически заполненных L1+.
- SSTable scan bounded-декодирует выбранные таблицы целиком перед merge.
- Нет block cache, table cache и background workers.
- Snapshot не сериализуется и действует только в создавшем его open handle.
- Нет backward iterator, transactions и column-family API.
- Inventory не удаляет orphan/obsolete/unknown файлы автоматически.

## Документация

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — границы компонентов;
- [`docs/LSM_READ_PATH_V1.md`](docs/LSM_READ_PATH_V1.md) — порядок source lookup и range merge;
- [`docs/SNAPSHOTS_AND_MVCC.md`](docs/SNAPSHOTS_AND_MVCC.md) — snapshot visibility;
- [`docs/FILE_INVENTORY_AND_VERIFY.md`](docs/FILE_INVENTORY_AND_VERIFY.md) — recovery inventory и verify;
- [`docs/formats/SSTABLE_V1.md`](docs/formats/SSTABLE_V1.md) — SSTable V1;
- [`docs/formats/MANIFEST_V1.md`](docs/formats/MANIFEST_V1.md) — MANIFEST V1;
- [`docs/formats/WAL_V1.md`](docs/formats/WAL_V1.md) — WAL V1;
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md) — проверяемые инварианты;
- [`docs/adr`](docs/adr) — архитектурные решения.

## Лицензия

MIT.
