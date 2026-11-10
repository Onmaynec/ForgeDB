# ForgeDB v0.6.0 — Cache and Performance

Статус: GitHub Pre-release.

ForgeDB v0.6.0 ускоряет обычные point reads и validation конфликтов MVCC-транзакций. Вместо повторного декодирования целого SSTable используется проверенный индекс и Bloom filter. Добавлен bounded LRU для декодированных блоков и консервативная очистка файлов, уже исключённых из live MANIFEST.

## Основные изменения

- Snapshot-aware point lookup через SSTable index с Bloom filter: декодируются только data blocks, которые могут содержать нужный ключ.
- Корректное чтение старых MVCC-версий и tombstones, когда версии одного user key лежат в нескольких соседних blocks.
- Binary search по непересекающимся key ranges L1+.
- Optimistic transaction write-write validation по newest committed sequence через индекс SST, без full-table materialization.
- `BlockCache`: LRU decoded-block cache, размер управляется `Options::block_cache_size` (0 отключает кеширование).
- `Database::cache_stats()` возвращает `CacheStats`: hits, misses, evictions, bypasses, число и объём cached blocks.
- Public forward `DbIterator` удерживает `Arc<VersionSet>` до завершения scan.
- После synced MANIFEST compaction installation удалённые input SST можно поставить в process-local retirement queue. Каждый retired файл отслеживает свой `Weak<TableReader>`; пока reader существует, physical cleanup откладывается.
- `Database::cleanup_obsolete_sstables()` удаляет только доказанно retired inputs из этого open handle и возвращает `CleanupReport` с результатом. CLI `compact` вызывает очистку после compaction.
- Добавлен benchmark `point_lookup_bench` без внешних зависимостей и короткий smoke-run в Linux/Windows CI.
- Добавлены регрессионные тесты на cache eviction/disabled, snapshot reads через data block boundaries, conflict detection, active iterator pins, восстановление и orphan-file preservation.
- Документация: `docs/CACHE_AND_PERFORMANCE.md`, ADR 0012/0013, обновлённые архитектура, read/write path, recovery и compatibility.

## Пример API

```rust
use forgedb::{Database, Options, Result, WriteOptions};

fn main() -> Result<()> {
    let db = Database::open("./data", Options::default())?;
    db.put(b"account:1", b"100", WriteOptions::sync())?;

    let stats = db.cache_stats()?;
    println!("cache hits={}, misses={}", stats.hits, stats.misses);

    db.compact_range(None, None)?;
    let cleanup = db.cleanup_obsolete_sstables()?;
    println!("removed SSTables: {}", cleanup.removed.len());
    db.close()
}
```

## Измерения

```bash
cargo run --locked --release -p forgedb --example point_lookup_bench -- 8192 50000
```

Benchmark создаёт временную базу, проверяет reads, выводит cold/warm durations и cache counters. В выпуске намеренно нет неподтверждённых процентов ускорения: полноценное сравнение с v0.5.0 требует одинакового оборудования, набора запросов и конфигурации.

## Persisted compatibility

**Изменений форматов нет.**

| Формат | Reader | Writer |
|---|---|---|
| IDENTITY | V1 | V1 |
| Internal Key | V1 | V1 |
| WAL | V1 | V1 |
| SSTable | V1 | V1 |
| MANIFEST | V1/V2 | V2 |
| CURRENT | V1 | V1 |

Cache хранится в памяти. Transaction commit продолжает пользоваться WAL V1 `WriteBatch` и snapshot isolation v0.5.0. Compaction по-прежнему публикует topology через synced MANIFEST edit.

## Очистка файлов и восстановление

Cleanup допустим только для SSTable, удалённого из live topology успешно синхронизированным MANIFEST edit. Пока `TableReader` жив, cleanup возвращает deferred, в том числе когда его удерживает старый iterator. Неудачное удаление оставляет объект на диске, не меняя authoritative state.

Очередь retired SST хранится **только в текущем процессе**. После restart ForgeDB не удаляет произвольные orphan-файлы: подтверждения права их удалить уже нет. Backup, unknown и user files normal recovery/cleanup не затрагивает.

## Границы v0.6.0

- `TableReader::open` всё ещё eagerly читает физические SSTable bytes в память. Предел `block_cache_size` ограничивает decoded block residency, **не общий RSS**. `table_cache_capacity` пока не реализует lazy eviction reader buffers.
- Range scan использует отдельный streaming cursor, не block cache point reads.
- Обязательная автоматическая restart-aware garbage collection и background compaction worker отсутствуют.
- Compaction synchronous, merge bounded в `max_compaction_bytes`, но materialized в пределах этого бюджета.
- MVCC isolation — snapshot isolation, не serializable; write skew всё ещё возможен.

## Проверки

Feature PR [#21](https://github.com/Onmaynec/ForgeDB/pull/21) прошёл весь CI [run 42787135076](https://github.com/Onmaynec/ForgeDB/actions/runs/42787135076): Ubuntu stable, Windows stable, Rust 1.89, strict Clippy, workspace tests, doctests, release build, benchmark smoke. Release workflow отдельно повторяет Linux gate до создания тега/Pre-release.

Подробнее: `docs/CACHE_AND_PERFORMANCE.md`, `docs/COMPACTION.md`, `docs/adr/0012-indexed-sst-lookup-and-cache.md` и `docs/adr/0013-reference-aware-sst-retirement.md`.
