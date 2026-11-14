# ForgeDB 1.0.0 — критерии стабильного выпуска

## Поддерживаемый продукт

**ForgeDB v1.0.0** — локальный встраиваемый LSM key-value движок на Rust со встроенным CLI. Проверяемая целевая среда: **Windows x86-64** и **Linux x86-64**, текущий Rust stable и **MSRV Rust 1.89**. Сеть, сервер, SQL, TLS, clustering или high availability не заявлены. Репозиторий использует MIT License.

Функции stable API: `Database` и `ColumnFamilies`, `get/put/delete`, atomic `WriteBatch` в пределах одной family, snapshot-aware point GET и range SCAN, optimistic snapshot-isolation transactions с конфликтами записи, flush/compaction, LSM WAL/SST/MANIFEST, quick/deep verification, recovery plan и FQL CLI.

CLI: `put/get/delete/scan/transaction/cf/fql/shell/compact/verify/inventory/inspect`, с v1.0.0 — `version`, `doctor`, `export` и `restore`. Read-only `get/scan/doctor` не создают отсутствующий каталог. `doctor --deep` выполняет проверку без ремонта, удаления orphan-файлов или перезаписи MANIFEST.

**Двоичный логический экспорт** переносит действующие named families, бинарные key/value и проверяет CRC32C. Backup/restore **не** заменяет полный физический checkpoint и не создаёт согласованный cross-family snapshot. Операции restore разрешены только в новый каталог. См. `docs/formats/LOGICAL_ARCHIVE_V1.md`.

## Перечень гарантий и ограничений

| Область | Поддержано в v1.0.0 | Ограничение |
|---|---|---|
| Durable writes | `WriteOptions::sync()`, WAL V1, recovery | Режимы no-sync/DisableWal имеют более слабые durability contracts |
| Atomicity | WriteBatch и transaction commit в **одной** family | Нет mixed-family atomic commit, Issue #35 |
| Snapshots | Snapshot на одну family, последовательное range scan | Нет cross-family consistent snapshot |
| Persisted compatibility | IDENTITY V1, CURRENT V1, WAL V1, SSTable V1, MANIFEST V1 read/V2 write, CF_REGISTRY V1 | Нельзя открывать будущие неизвестные форматы |
| FD budget | Bounded resident SST read LRU внутри одной DB | Не process-global quota (metadata validation, WAL/MANIFEST, outputs, multiple child DB), Issue #31 |
| Recovery & verify | CRC32C, file inventory, read-only recovery plan, deep verify | Авто-удаление неизвестных/orphan файлов запрещено; нет cross-restart GC |
| Backup/restore | Logical archive V1 + CRC32C + staged restore | Нет шифрования, подписи, онлайн атомарного checkpoint нескольких families |
| Distribution | Исходники GitHub + Windows/Linux x86-64 CLI archives и SHA-256 | Имя `forgedb` занято на crates.io, эта версия **не** опубликована от имени проекта в registry, Issue #27 |
| Security | Corruption detection, strict bounds, local process lock | Не encrypted, не authenticated/tamper-proof, не защищает от hostile local admin |
| Benchmark | Измеримый local `performance_bench` + CI smoke/full workload | Нет заявленного SLA/универсального ускорения; ignored 1024 SST stress не обязательный CI |

## Минимальный приемочный gate

Перед GA должны успешно пройти **на точном commit**:

1. Rustfmt и Clippy `-D warnings`, workspace tests/all features и doctests;
2. Cargo release build **и** `cargo doc --no-deps` на Windows stable и Ubuntu stable;
3. Clippy/tests на Rust 1.89;
4. Crash/durability/recovery/corruption/MVCC/Column Families/FQL/CLI tests, включая archive V1 checksum/future-version/recovery, doctor/read-only отсутствие созданных файлов и L1+ bound/snapshot regressions;
5. Benchmark smoke и Linux full eviction workload с реальными `evictions > 0` (это smoke, не performance SLA);
6. Отдельный проверенный release PR, аннотированный **v1.0.0 tag**, указывающий на release merge commit в `main`;
7. Повторный Linux release validation; сборка Windows и Linux бинарников **из того же тега**, end-to-end smoke обоих бинарников, SHA-256 и проверка состава архивов;
8. GitHub release первоначально **draft**, становится stable (`prerelease=false`, `draft=false`) **только после** загрузки четырёх проверенных файлов. GitHub API должен подтверждать все `uploaded`.

## Нерешённые работы

- [#31](https://github.com/Onmaynec/ForgeDB/issues/31) — глобальная квота OS descriptors и отдельные временные WAL/MANIFEST/compaction/verify handles.
- [#35](https://github.com/Onmaynec/ForgeDB/issues/35) — единый WAL/MANIFEST V3 и mixed-CF atomicity.
- [#27](https://github.com/Onmaynec/ForgeDB/issues/27) — безопасное название crates.io package.

Они не закрываются автоматически при достижении номера `1.0.0`. В стабильный контракт входят только реализованные и проверенные функции. Ограничения должны оставаться видны в README и release notes.
