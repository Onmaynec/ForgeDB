# Контракт проверки ForgeDB v0.7.0

Каждое изменение core, persisted-форматов или recovery обязано проходить GitHub Actions на Linux, Windows и минимально поддерживаемой версии Rust 1.89. Успешный compile не заменяет unit, model, integration и crash tests.

## Локальная проверка

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked --workspace --doc
cargo build --locked --workspace --release
```

MSRV jobs:

```bash
cargo +1.89 clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo +1.89 test --locked --workspace --all-features
```

Для локальной разработки есть `scripts/check.sh` и `scripts/check.ps1`.

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
