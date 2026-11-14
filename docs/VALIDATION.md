# Контракт проверки ForgeDB v1.0.0 Stable

Каждое изменение core, persisted-форматов или recovery обязано проходить GitHub Actions на Linux, Windows и минимально поддерживаемой версии Rust 1.89. Успешный compile не заменяет unit, model, integration и crash tests.

## Локальная проверка

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked --workspace --doc
cargo doc --locked --workspace --no-deps
cargo build --locked --workspace --release
```

MSRV jobs:

```bash
cargo +1.89 clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo +1.89 test --locked --workspace --all-features
```

Для локальной разработки есть `scripts/check.sh` и `scripts/check.ps1`.

## Stable GA v1.0.0 — дополнительные обязательные проверки

- `crates/forgedb-cli/tests/stable_ga_cli.rs`: `forgedb version` возвращает 1.0.0; `doctor --deep` действительно проверяет существующую базу read-only; `get`/`scan` на отсутствующем каталоге не создают пустую базу.
- `export`/`restore`: три действующие families (в том числе цифровое имя), бинарный key с NUL/0xFF, value 128 KiB, удалённый key, проверка после reopen; попытка overwrite запрещена.
- Архивы с повреждённой CRC, truncation или future version отклоняются; частично восстановленный destination не публикуется и временная папка не остаётся.
- `crates/forgedb/tests/stable_range.rs`: L1+ binary range seek после forced compaction нескольких SST, проверка Included/Excluded/Unbounded bounds, старого snapshot, tombstone и reopen.
- CRC32C Castagnoli: test vector `123456789 -> 0xE3069283`.
- Release artifact smoke из **tag v1.0.0** на Windows/Linux: `version`, `put`, `doctor --deep`, `export`, `restore`, `get`. Проверка ZIP/TAR.GZ и их SHA-256 до публикации Stable.
- CI должен пройти на точном feature/release/main commit, Linux/Windows stable и Rust 1.89. Rustdoc build обязателен.
- В отличие от pre-release, stable GitHub Release сначала Draft, публичный статус допустим только после четырёх загруженных assets и зелёного release workflow.

См. [GA_READINESS.md](GA_READINESS.md), [LOGICAL_ARCHIVE_V1.md](formats/LOGICAL_ARCHIVE_V1.md).

## Проверки именованных Column Families v0.7.0

- Совместимый default DB: старый root остаётся доступным без миграции.
- Один user key в двух именованных и default family имеет независимые значения/удаления.
- Flush/compaction/reopen не меняют изоляцию и names registry.
- Optimistic transactions, snapshots, snapshot mismatch и conflicts проверяются внутри отдельной family.
- Registry checksum damage/future version/malformed name и missing registered directory должны давать ошибку, а не silently созданную пустую базу.
- `drop_family` не переиспользует ID и не удаляет физический каталог без подтверждения.
- Read-only не создаёт реестр, подкаталоги или WAL и запрещает create/drop/write.
- Partial create orphan directory не может стать active family и не уничтожается автоматически.
- CLI выполняет create/list/get/put/delete/drop в отдельных процессах, с reopen и проверкой числовых ID.
- `create/drop/open` сериализованы с `close()` одним registry mutex; операции после закрытия возвращают `Closed`.
- Symlink в reserved registry/CF path отклоняется; dangling orphan ID не переиспользуется.

## Проверки FQL V1 / shell v0.8.0

- Parser/lexer: неверное число аргументов, лишние statements в строке, invalid quoted/hex escapes, некорректный scan range, повторение FROM/TO/LIMIT, oversized statement/tokens.
- Statement parse завершён до открытия БД в one-shot FQL; ошибочная команда не создаёт empty database и не применяет частичный write.
- Binary bytes и quoted whitespace проходят `PUT`→`GET` с обратимым escaping.
- `shell --file`: семейства, BEGIN→own-write GET→ROLLBACK/COMMIT, reopen; ненулевой exit на строке ошибки, следующие команды не выполняются.
- `shell` из stdin: после parser failure сессия продолжает работу, ранее правильные записи доступны.
- EOF/EXIT/file error: незавершённые writes откатываются.
- `--read-only` запрещает write/DDL и не создаёт базу.
- SCAN streaming limit/bounds; запрет SCAN и USE/CREATE/DROP при active transaction.
- CI Ubuntu/Windows stable и Rust 1.89, strict Clippy -D warnings, форматирование, workspace tests, doctests и release build.

## Проверки SST handle-cache v0.8.1

- Capacity 1: два SST, отдельные point GET, LRU evict и reopen первого; open_handles не превышает 1.
- Переоткрытие DB после flush/recovery сохраняет значения, новый pool начинает с 0 resident FDs.
- Unit: header повреждён после eviction, последующий reopen отклоняет corruption; после восстановления того же SST чтение повторно работает.
- Старый streaming iterator удерживает `VersionSet`/TableReader при compaction; cleanup откладывает физическое удаление до release reference и на Windows закрывает pooled handle перед remove_file.
- MVCC snapshot и write-write optimistic transaction conflict не меняются после FD eviction.
- Возможное понижение throughput при mutex contention не скрывается, бенчмарк smoke не является сравнительным proof ускорения.
- Отдельные scan cursor, WAL, compaction, named CF FDs не объявляются частью process-wide бюджетирования; Issue #31 остаётся открытой.

## Проверки общего SST read pool v0.8.2

- 48 L0 SST по 10 ключей (несколько data blocks на файл), `table_cache_capacity=1`. Streaming iterator корректно возвращает 480 ключей и bytes в порядке сортировки, счётчик resident handles никогда не превышает capacity.
- Повторный GET к SST, уже вытесненному в ходе scan, принудительно проверяет `reopens`. Повторное открытие базы возвращает прежние 480 записей без миграции.
- Четыре concurrent GET threads и streaming scan: pool не превышает 1 resident handle; все результаты и snapshot semantics сохраняются.
- Compaction input использует тот же pool. Пока старый iterator жив, cleanup deferred; после release удаление retired SST проходит, orphan отсутствуют, всё читается.
- Дополнительный `#[ignore]` stress с 1024 SST выполняется вручную и **не считается пройденным** при обычном CI.
- Ограничение измеряется только для resident SST read pool: кратковременные init validation Files, writes, WAL/MANIFEST/LOCK, deep verification и sibling child databases вне cap. Нельзя выдавать это за process-wide FD limit.

## Производительность и регрессионные проверки v0.9.0

- `ordered_handle_lru_keeps_most_recent_file_and_maintains_exact_metrics`: бюджет 2, три distinct SST, повторное обращение к alpha делает его MRU; появление третьего файла вытесняет правильный LRU. Точные `hits/misses/evictions/reopens`, нет превышения capacity.
- `conflict_validation_binary_search_finds_versions_in_leveled_sst`: сотни записей, forced compaction, point reads из L1+, optimistic transaction conflict после BEGIN, reopen и latest.
- `mvcc_merge_discards_large_invisible_values_without_losing_snapshot_semantics`: 128-KiB values, множество overwrites, read old snapshot и latest scan, tombstone suppression.
- `narrow_range_scan_preserves_order_across_many_sst_without_handle_vector`: 48 SST, диапазон через границу двух SST, ключи внутри и вне half-open границ в правильном порядке.

- Обязателен старый CI набор: WAL/manifest/corruption/recovery, FQL, named Column Families, streaming cursor, Windows-safe cleanup, Rust 1.89.
- `cargo run --locked --release -p forgedb --example performance_bench -- 256 1024` — benchmark smoke на Ubuntu/Windows stable. Дополнительно Ubuntu выполняет полный workload `-- 8192 32768` и отвергает сценарий, в котором при 64 SST и `table_cache_capacity=8` не произошло ни одного eviction. Это проверка измерителя и задействованного пути, не подтверждение процентного ускорения.
- Полный workload `-- 8192 32768`, сравнение с одинаковой конфигурацией на одной машине, несколько прогонов, оценка p50/p95/p99 и scan throughput. Описан в [PERFORMANCE_BENCHMARK.md](PERFORMANCE_BENCHMARK.md).
- Дополнительный LRU индекс расходует память O(N) и синхронизацию под mutex; результаты реальной производительности не должны приписываться алгоритмическим оценкам без paired benchmark.

## Обязательные направления тестирования

- WAL atomic batch, checksum, partial tail, sequence gaps и replay;
- FLUSH: WAL rotation, previous/current recovery, retry после crash;
- SSTable: bounded decoding, checksums, block boundaries, Bloom filter;
- MANIFEST V1/V2 compatibility, migration, future-format rejection;
- snapshot visibility и MVCC retention;
- L0→L1 и Ln→Ln+1 compaction, user-key-safe output splitting;
- tombstone dropping только при bottommost/snapshot-safe условиях;
- duplicate/overlap/absent file detection;
- process-abort boundaries, reopen и file inventory/verify;
- Windows lock/sync/rename semantics и Rust 1.89.

Новые persisted-форматы требуют ADR, фиксированного layout, fixtures, migration/rollback и rejection tests.

## Проверки исправительного обновления v0.6.1

- fault injection при WAL/MANIFEST sync внутри close и последующее успешное закрытие;
- удержание LOCK после ошибочного close, idempotent repeated close и reopen;
- continuous writes с bounded memtable и automatic synchronous flush/retry;
- oversized batch не имеет частично опубликованных mutations;
- автоматическая безопасная очистка retired SST после compaction, защита активных scan cursors и snapshots;
- сохранение неизвестных и orphan SST после restart.

## Проверки v0.6.2 — lazy SST readers

- Instrumented random-access FileSystem: число прочитанных байтов при `TableReader::open` существенно меньше физического SSTable.
- Повторный snapshot-aware point read в cached block не выполняет новое физическое чтение.
- Existing split-key MVCC, tombstone, corruption, transaction newest-sequence validation и deep verify tests.
- Компакция, удержание TableReader/VersionSet итераторами и reference-aware удаление на Windows.
- Benchmark cold/warm с throughput и p50/p95/p99, без фиксированного speed threshold.

## Cache and performance checks

Дополнительно проверяются snapshot-aware индексный lookup через block boundary, cache eviction и disabled режим, возврат transaction conflicts после flush, iterator pinning при SST cleanup, snapshot retention при compaction и сохранение orphan-файлов после restart.

Короткий benchmark smoke-run выполняется на Linux и Windows без порога скорости:

```bash
cargo run --locked --release -p forgedb --example point_lookup_bench -- 256 1024
```

Для ручного сравнения cold/warm на большем наборе:

```bash
cargo run --locked --release -p forgedb --example point_lookup_bench -- 8192 50000
```

Нельзя выдавать результаты разных CPU/FS/toolchain как гарантированное ускорение.

## Release gate

Тег и GitHub Pre-release публикуются только после успешных канонических проверок, подтверждения версии workspace, обновления русских release notes и самостоятельной проверки recovery/durability impact. Нельзя обходить Clippy или отключать платформенный тест ради зелёного CI.

Проверенные run ID указываются в описании PR; статус тестов не переносится с другого commit. Политика выпусков: [RELEASE_POLICY.md](RELEASE_POLICY.md).
