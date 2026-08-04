# ForgeDB

ForgeDB — встраиваемый key-value storage engine на Rust. Проект ставит корректность,
явные гарантии durability, ограниченное использование ресурсов, версионируемые persisted-форматы
и обнаружение повреждений выше преждевременной оптимизации производительности.

## Текущий этап: v0.3.0 — Full LSM Read Path

`v0.3.0` разрабатывается как stacked draft поверх ещё не выпущенной `v0.2.0`. Ветка и PR не должны
считаться релизом до успешной Linux/Windows/Rust 1.89 матрицы.

В текущем draft реализованы WAL/SSTable/MANIFEST V1, MVCC memtables, двухфазный L0 flush, immutable
`Arc<VersionSet>`, snapshots, snapshot-aware point reads, forward range scan, block-at-a-time
`SstEntryCursor`, file inventory, recovery plan и quick/deep verify.

Код streaming path и его tests находятся в draft-ветке, но их успешная компиляция и выполнение ещё не подтверждены доступным CI runner.

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
        database.get_with_options(b"user:42", ReadOptions::from_snapshot(&snapshot))?,
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
cargo run -p forgedb-cli -- inspect recovery-plan ./data
cargo run -p forgedb-cli -- verify ./data --deep
```

`inspect recovery-plan` получает process lock, выбирает authoritative MANIFEST, показывает порядок WAL replay,
live SSTables, orphan/obsolete/temp/backup/user/unknown files, warnings, blockers и только предлагаемые cleanup actions.
Команда не выполняет rename или delete.

## Порядок чтения

Point lookup использует один sequence и проверяет mutable, immutable, L0 newest-first, затем L1+.
`Value` и `Tombstone` завершают поиск; только `Missing` разрешает перейти к следующему источнику.

Range scan объединяет sorted MVCC cursors через binary heap. Memtable cursors владеют frozen entry vectors.
Каждый SSTable cursor удерживает проверенный index и entries только текущего data block; следующий block
читается через `seek + read_exact` после исчерпания текущего.

## Durability

- `Sync`: WAL frame синхронизируется до публикации batch в mutable memtable.
- `NoSync`: WAL frame записывается без обязательного физического sync перед возвратом.
- `DisableWal`: batch существует только в памяти до flush и может исчезнуть при process crash.

Flush публикует WAL rotation в MANIFEST, строит и синхронизирует SSTable, отдельным VersionEdit устанавливает
таблицу и только затем делает старый WAL obsolete.

## Проверка

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

Release gate требует Ubuntu stable, Windows stable и Rust 1.89. Run `30878893688` завершил все три jobs
до checkout с `steps=null`, поэтому текущий stacked draft не имеет подтверждённого зелёного результата.

## Ограничения v0.3.0

- `v0.2.0` и `v0.3.0` ещё не выпущены.
- Нет compaction и физически заполненных L1+.
- Index каждого выбранного SSTable загружается целиком; data blocks читаются по одному.
- Нет block cache, table cache и background workers.
- Snapshot действует только в создавшем его open handle.
- Нет backward iterator, transactions и column-family API.
- Inventory и recovery plan не изменяют файлы автоматически.
- Streaming `Database` временно оформлен как public wrapper над storage core; типы должны быть объединены после доступного compiler/refactor pass без изменения API.

## Документация

- [`docs/LSM_READ_PATH_V1.md`](docs/LSM_READ_PATH_V1.md);
- [`docs/SNAPSHOTS_AND_MVCC.md`](docs/SNAPSHOTS_AND_MVCC.md);
- [`docs/FILE_INVENTORY_AND_VERIFY.md`](docs/FILE_INVENTORY_AND_VERIFY.md);
- [`docs/formats/SSTABLE_V1.md`](docs/formats/SSTABLE_V1.md);
- [`docs/formats/MANIFEST_V1.md`](docs/formats/MANIFEST_V1.md);
- [`docs/formats/WAL_V1.md`](docs/formats/WAL_V1.md);
- [`docs/INVARIANTS.md`](docs/INVARIANTS.md).

## Лицензия

MIT.
