# ForgeDB v0.6.2 — Memory & Reliability

Статус: GitHub Pre-release.

ForgeDB v0.6.2 уменьшает память, расходуемую при открытии и индексном чтении SSTable: физические SST-файлы больше не загружаются целиком в `TableReader::open`. Служебные блоки проверяются при открытии, data blocks читаются по запросу через random-access FileSystem с CRC32C. Прежние persisted-форматы и поведение транзакций сохраняются.

## Что изменилось

- `TableReader::open` проверяет размер из MANIFEST, фиксированные header/footer и checksummed index/filter/properties без создания `Vec<u8>` размером в SSTable.
- Point-read data blocks загружаются отдельно по проверенным offsets. `get`, snapshot-aware `get_at_cached` и newest-sequence transaction conflict lookup используют тот же механизм.
- Один открытый reader handle удерживается на lifetime `TableReader` и разделяется клонами под mutex; старые `VersionSet`/читатели сохраняют защиту от удаления SST во время compaction cleanup.
- CRC32C, compression type и reserved trailer fields проверяются при чтении физического блока; ошибка не попадает в LRU decoded cache.
- Benchmark `point_lookup_bench` дополнен throughput в ops/s и empirical p50/p95/p99 (ns) для cold/warm проходов. Ускорение относительно старых версий не заявляется без baseline на одинаковом оборудовании.

## Как проверить

```bash
cargo run --locked --release -p forgedb --example point_lookup_bench -- 8192 50000
```

Результаты зависят от файловой системы, OS page cache, объёма набора данных, размеров SST, параметров кэша и планировщика. Небольшой benchmark smoke используется в CI только для проверки работоспособности, не как speed SLA.

## Совместимость и безопасность

| Формат | Чтение | Запись |
|---|---|---|
| IDENTITY | V1 | V1 |
| Internal Key | V1 | V1 |
| WAL | V1 | V1 |
| SSTable | V1 | V1 |
| MANIFEST | V1/V2 | V2 |
| CURRENT | V1 | V1 |

Изменений persisted layout и migration нет. Сохраняются WAL durability modes, optimistic snapshot isolation, write-write validation, reference-aware cleanup из v0.6.1 и поведение `close()` после sync errors.

## Что пока не сделано

- `Options::block_cache_size` ограничивает только decoded data-block cache, не весь RSS: index/Bloom/properties, временные I/O buffers, memtables и прочие расходы памяти учитываются отдельно.
- `table_cache_capacity` пока не управляет количеством открытых SST file handles; активные SST readers удерживают физические дескрипторы, поэтому очень большие базы могут столкнуться с лимитом FD.
- Проверка целостности data blocks выполняется при чтении или через `verify --deep`, а не полным сканированием каждого SST при open. Уже кэшированный decoded block не перечитывается с диска на каждом cache hit.
- Автоматическая cross-restart очистка orphan SST, фоновые compaction workers, Column Families и serializable transactions не добавлены.

## Проверки

Feature head `04bfd605b961d57447e3b9baed3b177228c60d8b`, [CI #43548913230](https://github.com/Onmaynec/ForgeDB/actions/runs/43548913230) — success:

- Ubuntu stable: rustfmt, strict Clippy `-D warnings`, workspace tests, doctests, release build, benchmark smoke.
- Windows stable: те же проверки.
- Ubuntu Rust 1.89: strict Clippy и workspace tests.

Добавлены измерительный тест, подтверждающий отсутствие full-file SST reads при открытии, проверка чтения data block с кешированием без повторного I/O и fault injection повреждения данных во время random-access чтения. Release workflow отдельно повторяет Linux gate перед публикацией annotated tag.

## Готовый CLI для Windows и Linux

В Assets релиза размещаются архивы x86-64 CLI с отдельными SHA-256 checksum:

- `forgedb-v0.6.2-windows-x86_64.zip` — `forgedb.exe`;
- `forgedb-v0.6.2-linux-x86_64.tar.gz` — `forgedb`.

Пример первого запуска после распаковки (Windows: `./forgedb.exe`):

```bash
./forgedb put ./data hello world
./forgedb get ./data hello
./forgedb scan ./data
./forgedb verify ./data --deep
```

Для использования CLI не нужен Rust toolchain. Детали проверки архива: `docs/QUICKSTART.md`.

## Документация

- `docs/releases/V0.6.2.md`;
- `docs/adr/0014-lazy-sst-readers.md`;
- `docs/CACHE_AND_PERFORMANCE.md`;
- `docs/READ_PATH.md`, `docs/OPTIONS.md`, `docs/INVARIANTS.md`.
