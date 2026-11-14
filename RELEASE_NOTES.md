# ForgeDB v0.8.1 — SST Handle Cache & Reliability

Статус: GitHub Pre-release.

Исправительный выпуск v0.8.1 сокращает число постоянно открытых SST-файлов при обычных точечных чтениях и исправляет порядок освобождения дескрипторов перед очисткой retired SSTable на Windows. Форматы хранения и совместимость баз v0.8.0 сохранены.

## Новый SST handle cache

Раньше `TableReader` удерживал отдельный физический файловый дескриптор до окончания жизни `VersionSet`. При большом количестве SSTable это могло исчерпать лимит открытых файлов, несмотря на lazy data block reads.

Теперь `Options::table_cache_capacity` действительно задаёт максимальное число одновременно хранимых native **point-read** SST handles внутри одной базы (по умолчанию 512). Кэш использует LRU eviction. Дескриптор открывается при первом физическом чтении data block и закрывается при вытеснении. Все поколения `VersionSet`, включая старые snapshots, используют общий pool.

При повторном открытии файла проверяются его размер, database identity, номер SST, диапазон sequence numbers и checksum заголовка. Прочитанные data blocks проходят существующую проверку BlockHandle, CRC32C и decoder. Физический FD закрывается *перед* открытием следующего при заполненном pool. Один mutex обеспечивает строго ограниченную вместимость, но может уменьшить параллелизм точечных SST чтений.

## Метрики

Доступна диагностика через `Database::table_handle_stats()` или `ColumnFamilies::table_handle_stats(id)`:

```rust
let stats = database.table_handle_stats()?;
println!(
    "open={}/{} hits={} misses={} evictions={} reopens={}",
    stats.open_handles, stats.capacity, stats.hits, stats.misses,
    stats.evictions, stats.reopens
);
```

`Database::cache_stats()` по-прежнему относится к отдельному кэшу decoded data blocks в памяти. Значение `block_cache_size` никак не подменяется новым лимитом.

## Windows-safe cleanup

После compaction удаляются только файлы, которые были выведены из подтверждённой MANIFEST topology и более не удерживаются `TableReader`. Очистка теперь сначала закрывает сохранённый в pool файловый дескриптор, а затем вызывает `remove_file`, устраняя sharing violation для такого read handle на Windows. При живом старом iterator/snapshot reader физическое удаление по-прежнему откладывается.

Неизвестные orphan SST после restart не удаляются автоматически.

## Проверки

Feature PR [#40](https://github.com/Onmaynec/ForgeDB/pull/40), head `d3a39c211efb1865fd8dd10b92bdfcb63e97d58e`, [CI #43558647759](https://github.com/Onmaynec/ForgeDB/actions/runs/43558647759): Ubuntu stable, Windows stable, Rust 1.89 — **success**.

Добавлены тесты:
- `point_read_budget_one_evicts_and_reopens_without_losing_values` — capacity=1, несколько SST, LRU eviction, restart;
- `native_handle_cache_reopens_with_bounded_fd_and_rechecks_header` — повреждённый header после eviction отвергается, восстановленный файл снова читается;
- `pinned_scan_survives_compaction_and_retirement_evicts_windows_handles` — reference-aware compaction/cleanup и порядок удаления;
- `transaction_conflicts_and_snapshot_reads_survive_fd_eviction` — MVCC snapshots и optimistic transaction conflicts после reopen.

Предварительные CI проверяют rustfmt, Clippy `-D warnings`, workspace tests, doctests, release build и benchmark smoke; release workflow повторяет Linux gate.

## Честные ограничения

**Это не общий лимит всех файловых дескрипторов процесса.** `table_cache_capacity` учитывает только point-read pool одного `Database`; streaming range cursors, compaction, temporary writes, WAL/MANIFEST/LOCK и другие named Column Families работают с отдельными дескрипторами. Поэтому общая [Issue #31](https://github.com/Onmaynec/ForgeDB/issues/31) остаётся открытой.

Скорость запросов и RSS не объявляются улучшенными без воспроизводимого сравнения: shared mutex сериализует point-read physical I/O и имеет trade-off по throughput. Unified Column Families и mixed-CF atomic WAL из [Issue #35](https://github.com/Onmaynec/ForgeDB/issues/35) не входят в v0.8.1.

Persisted IDENTITY V1, WAL V1, Internal Key V1, SSTable V1, MANIFEST writer V2/reader V1–V2, CURRENT V1 и CF_REGISTRY V1 не менялись. Миграция не требуется.

## CLI для Windows и Linux

GitHub Pre-release публикует два x86-64 архива и отдельные SHA-256 checksums:

- `forgedb-v0.8.1-windows-x86_64.zip` — `forgedb.exe`;
- `forgedb-v0.8.1-linux-x86_64.tar.gz` — `forgedb`.

Существующие `forgedb fql`, `forgedb shell`, `forgedb cf` и остальные CLI команды не изменены. Инструкция по установке и проверке checksum: `docs/QUICKSTART.md`.

## Документация

- `docs/releases/V0.8.1.md`;
- `docs/adr/0017-bounded-sst-handle-cache.md`;
- `docs/CACHE_AND_PERFORMANCE.md`;
- `docs/OPTIONS.md`;
- `docs/INVARIANTS.md`;
- `CHANGELOG.md`.
