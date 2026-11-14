# ADR 0020 — Stable GA, переносимый архив и диагностический CLI

- Статус: принято для v1.0.0
- Дата: 2026-11-14

## Решение о продукте

Версия v1.0.0 фиксирует стабильный API локального embedded LSM key-value движка и CLI, а не обещает серверную DBMS либо полноту PostgreSQL/RocksDB. Поддержка ограничена Windows/Linux x86-64 с MSRV Rust 1.89. API и on-disk форматы перестают меняться без публичной политики версий и миграции.

Стабильная публикация до этого блокировалась pre-release gate и отсутствием готового штатного CLI workflow для реального GA. Раньше GitHub Pre-release создавался до завершения сборки двух платформ: частично опубликованный release мог существовать. В v1.0 workflow сначала проверяет точно соответствующий main commit и создаёт immutable annotated tag, затем отдельно строит/тестирует оба CLI и проверяет SHA-256, загружает в черновик и лишь после полного набора делает release публичным stable.

## Portable logical archive

Физическая копия живых WAL/SST/MANIFEST без согласованного checkpoint может дать нечитаемое состояние. Поэтому v1.0 не добавляет команду `cp -r` и не объявляет её backup. Вместо этого CLI `export` сканирует действующие families через MVCC snapshots, формирует frame-based `LOGICAL_ARCHIVE V1`, CRC32C проверяет весь архив. `restore` в новый каталог сначала применяет данные во временную базу, сверяет checksum и EOF, закрывает WAL/записи и только после этого публикует каталог rename.

Этот механизм **не** объединяет sequence numbers разных named families: snapshots захватываются раздельно. API реестра может содержать пропущенные числовые ID, поэтому на restore сохраняются имена/данные, но не ID. Сохраняются только логические видимые keys, не история tombstones/MVCC и не пользовательские unknown-файлы. Неизвестные будущие версии формата отклоняются. CRC не защищает от злонамеренной модификации, архив не шифрует данные.

Полное описание полей: `docs/formats/LOGICAL_ARCHIVE_V1.md`.

## Диагностика и безопасность по умолчанию

Раньше команды `get` и `scan` в CLI могли открыть несуществующий path с `Options::default()`, создавая пустую базу без намерения записи. В v1.0 они используют `read_only=true, create_if_missing=false`, как `verify`, `inventory` и FQL `--read-only`. `doctor` сначала вызывает `RecoveryPlan::inspect` под lock, затем выпускает lock перед отдельным read-only `Database::verify`. Проверки могут обнаружить blockers/corruption, но не выполняют cleanup, migration, mutation, `flush` или speculative repair.

## L1+ scan planning

Для streaming scan `VersionSet::files_for_scan` теперь получает консервативные lower/upper key bounds, выбирает диапазон кандидатов L1+ через `partition_point` по `largest_user_key` и `take_while` по `smallest_user_key`. O(log N + K) candidate planning на каждом непересекающемся уровне. L0 оставлен newest-first с полным overlap-check, поскольку диапазоны перекрываются. После pruning все кандидаты ещё раз проверяются через `OwnedRange::overlaps`, учитывающий `Included/Excluded` границы. Изменение не затрагивает MVCC, tombstone, ownership `VersionSet` и SST layouts.

## Ограничения

Global process FD (#31), atomic cross-family writes (#35) и crates.io package naming (#27) остаются открытыми. Нет API для incremental online physical backup, серверного состояния, encryption, ACL и криптографической аутентификации архива. Для этих требований нужен отдельный milestone и совместимая migration policy.

## Проверки

`crates/forgedb-cli/tests/stable_ga_cli.rs` и `crates/forgedb/tests/stable_range.rs`, весь существующий workspace CI, независимая Rust 1.89 проверка, docs build, end-to-end smoke готовых Windows/Linux CLI, tagged release и actual assets. Протокол публикации описан в `docs/RELEASE_POLICY.md` и `docs/GA_READINESS.md`.
