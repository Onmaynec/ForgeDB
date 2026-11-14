# ForgeDB v0.9.0 — Performance Engine

Статус: GitHub Pre-release.

Этот выпуск посвящён оптимизации горячих путей embedded key-value движка. Устранены лишний линейный поиск в SST handle LRU, копирования больших значений при MVCC merge, полный проход по непересекающимся SST во время проверки конфликтов транзакций и временное клонирование всех SST handles при запуске `SCAN`. Форматы данных на диске, транзакционная изоляция и существующий CLI сохранены.

## 1. Индексированный SST handle LRU

Native SST File pool теперь использует два согласованных индекса: `BTreeMap<file_number, CachedHandle>` и `BTreeSet<(last_access, file_number)>`. При заполненном пуле выбор жертвы больше не сканирует все N открытых файлов (`O(N)`), а берёт oldest из ordered index за `O(log N)`. Hit/miss/retirement обновляют индексы под общим mutex. Кэш не оставляет более `table_cache_capacity` resident File, включая параллельные GET, streaming SCAN и compaction inputs одного Database.

**Обмен:** появляется одна служебная LRU запись на resident File, то есть O(N) дополнительной памяти, и обновление ordered index O(log N) на каждом hit. При маленьком количестве файлов отдельные операции могут не ускориться. Семантика Windows-safe cleanup остаётся: физическое удаление retired SST после освобождения readers и закрытия pooled FD.

## 2. Меньше копирований в MVCC merge

`DbIterator::advance_source` проверяет строгий порядок только по `InternalKey`, не клонирует целый `TableEntry.value`. `consume_item` пропускает copy value у истории ключа, которая не видна текущему snapshot или уже перекрыта найденной новой видимой записью. Ошибки нарушенного порядка, MVCC sequence, tombstones, snapshot visibility и `SCAN LIMIT` продолжают проверяться прежним способом.

Это особенно актуально для больших values с многочисленными перезаписями, но не заменяет обязательное декодирование blocks и checksum validation.

## 3. Быстрее выбор SST при проверке конфликтов

`VersionSet::latest_sequence_after` продолжает проверять перекрывающиеся L0 (newest-first), но использует cutoff по `max(after_snapshot, newest_found)`, если SST физически не могла содержать новую sequence. На непересекающихся L1+ выбирается единственный возможный кандидат через `partition_point`, вместо линейного обхода всех SST в уровне. Оптимистическая транзакция по-прежнему не может записать stale value поверх более нового committed изменения.

## 4. Легче запуск узких SCAN

`VersionSet::files_for_scan()` возвращает заимствованный iterator по L0 newest-first и L1+ в порядке сортировки. Раньше `StreamingDatabase::scan` создавал временный `Vec<TableHandle>` для всех N live SST и клонировал каждый `Arc<TableReader>` до фильтрации range. Теперь сканирование проверяет overlaps непосредственно и создаёт курсоры только для подходящих SST.

Временный список O(N) устранён, но проверка перекрытий по SST остаётся O(N): бинарный переход к первому диапазону `SCAN` не заявляется.

## Регрессионные тесты

- `ordered_handle_lru_keeps_most_recent_file_and_maintains_exact_metrics`: capacity 2, 3 SST, проверка MRU/LRU, точных cache hits, misses, evictions, reopens.
- `conflict_validation_binary_search_finds_versions_in_leveled_sst`: 480 ключей, многослойная compaction, актуальные GET, optimistic transaction conflict, reopen.
- `mvcc_merge_discards_large_invisible_values_without_losing_snapshot_semantics`: values 128 KiB, 16 исторических перезаписей, tombstones, old snapshot и latest scan.
- `narrow_range_scan_preserves_order_across_many_sst_without_handle_vector`: 48 SST и узкий half-open range через соседние SST.
- Весь прежний набор WAL/durability/recovery/crash, checksum/corruption, Column Families, FQL и Windows cleanup.

Feature PR [#46](https://github.com/Onmaynec/ForgeDB/pull/46); feature head `78e7025c9807f3d9015819bb30721ab556e69c50`. [CI #43562510705](https://github.com/Onmaynec/ForgeDB/actions/runs/43562510705) успешно прошёл Ubuntu stable, Windows stable, Rust 1.89, rustfmt, strict Clippy, workspace tests/doctests, release build и оба benchmark режима.

## Измерения: один воспроизводимый пример, не сравнение с прошлой версией

На Ubuntu GitHub Actions, CI #43562510705, вызов:

```bash
cargo run --locked --release -p forgedb --example performance_bench -- 8192 32768
```

Получено на **одном** CI runner:

| Показатель | Результат |
|---|---:|
| 8192 ключей, value 256 B, 64 SST, block_cache_size=0 | конфигурация |
| Resident SST handles | 8 из 8 |
| Evictions / reopens | 65 589 / 65 533 |
| Cold GET | 141 346 ops/sec; p50 6839 ns, p95 7423 ns, p99 9870 ns |
| Warm GET | 142 421 ops/sec; p50 6820 ns, p95 7182 ns, p99 10309 ns |
| Full SCAN | 8192 entries / 15,522 ms; 527 771 entries/sec |

Здесь `block_cache_size=0` намеренно отключает decoded-block cache, чтобы путь действительно открывал SST через LRU. В полной нагрузке CI проверяет наличие evictions. Это **не** A/B с v0.8.2 на том же железе: процент ускорения из этих цифр не выводится, переносить их в SLA нельзя. Файловый кэш, планировщик, дисковая подсистема и параллельная нагрузка влияют на результат. Методика повторов и сравнения: `docs/PERFORMANCE_BENCHMARK.md`.

## Совместимость и границы

Persisted IDENTITY V1, WAL V1, SSTable V1, MANIFEST reader V1–V2 / writer V2, CURRENT V1 и CF_REGISTRY V1 не меняются. Миграция базы не требуется. FQL/CLI commands `forgedb fql`, `forgedb shell`, `forgedb cf` и существующие API остаются совместимыми.

`table_cache_capacity` не гарантирует process-wide FD limit: initial validation Files, SST writers/outputs, WAL/MANIFEST/LOCK, deep verify, другие named child databases используют собственные handles. [Issue #31](https://github.com/Onmaynec/ForgeDB/issues/31) остаётся открытой. Глобальная mixed-family atomic WAL/MANIFEST архитектура [#35](https://github.com/Onmaynec/ForgeDB/issues/35) в этот выпуск не входит.

## Готовые CLI архивы

В GitHub Pre-release публикуются Windows x86-64 ZIP (`forgedb.exe`), Linux x86-64 TAR.GZ (`forgedb`) и отдельные SHA-256 файлы:

- `forgedb-v0.9.0-windows-x86_64.zip`;
- `forgedb-v0.9.0-linux-x86_64.tar.gz`.

Документы: `docs/releases/V0.9.0.md`, `docs/adr/0019-performance-engine.md`, `docs/PERFORMANCE_BENCHMARK.md`, `docs/CACHE_AND_PERFORMANCE.md`, `docs/INVARIANTS.md`, `CHANGELOG.md`.
