# ForgeDB v0.4.0 — Leveled Compaction

Статус выпуска: GitHub Pre-release.

ForgeDB v0.4.0 превращает L1+ из validated read model в persisted LSM topology: добавлены leveled compaction, MVCC-aware retention, conservative tombstone dropping, output splitting, L0 backpressure и MANIFEST V2.

## Основные изменения

- physical L0 → L1 и Ln → Ln+1 compaction;
- picker с L0 overlap closure и size targets для L1+;
- bounded compaction budget через `max_compaction_bytes`;
- MVCC retention с `oldest_active_snapshot_sequence`;
- bottommost tombstone dropping только при snapshot-safe условиях;
- output splitting без разделения versions одного user key;
- atomic compaction installation одним MANIFEST edit;
- durable reservation file numbers до создания outputs;
- persisted L1+ metadata;
- MANIFEST V2 с read compatibility MANIFEST V1;
- crash-safe automatic V1 → V2 rotation при read-write open;
- read-only V1 open без migration;
- L0 slowdown/stop backpressure;
- automatic compaction после public `flush()`;
- public `Database::compact_range`;
- CLI `forgedb compact <database> [start] [end-exclusive]`;
- reopen test после L0 → L1 installation;
- unit tests retention, bottommost tombstone и user-key-safe output splitting.

## MANIFEST V2

SSTable и WAL остаются V1. Новый persisted contract нужен только для authoritative level placement.

| Формат | Read support | Write support |
|---|---|---|
| IDENTITY | 1 | 1 |
| Internal Key | 1 | 1 |
| WAL | 1 | 1 |
| SSTable | 1 | 1 |
| MANIFEST | 1, 2 | 2 |
| CURRENT | 1 | 1 |

ForgeDB v0.3.x корректно отклоняет MANIFEST V2 как future format. ForgeDB v0.4.x читает V1/V2 и пишет V2.

Writable open V1 базы создаёт новый V2 snapshot MANIFEST, синхронизирует его и только затем crash-safe заменяет `CURRENT`. V1 MANIFEST не изменяется in-place.

## Compaction correctness

Для каждого user key newest version сохраняется. Если существует active snapshot, compaction сохраняет все версии новее oldest snapshot и первую версию не новее него. Это сохраняет observable result всех активных snapshots.

Tombstone удаляется только в bottommost range и только если активному snapshot не требуется состояние до tombstone.

Compaction outputs сначала полностью создаются как SSTable V1. Затем один checksummed MANIFEST V2 edit атомарно удаляет input references и добавляет outputs в target level. До этого edit authoritative остаются inputs; после durable edit — outputs.

## Write pressure

- `l0_compaction_trigger` запускает compaction после flush;
- `l0_slowdown_trigger` добавляет bounded задержку перед writes;
- `l0_stop_trigger` требует synchronous compaction и возвращает `Busy`, если pressure не удалось снизить.

Задержка не выполняется под database state lock.

## Проверка выпуска

Tag и GitHub Pre-release создаются release workflow только после успешного:

- `cargo fmt --all --check`;
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`;
- `cargo test --locked --workspace --all-features`;
- `cargo test --locked --workspace --doc`;
- `cargo build --locked --workspace --release`.

Основной PR CI дополнительно проверяет Ubuntu, Windows и Rust 1.89.

## Известные ограничения

- compaction synchronous; отдельный background worker pool ещё не реализован;
- merge bounded `max_compaction_bytes`, но job materialize-ит выбранные entries внутри этого лимита;
- physical input SST после successful compaction не удаляются немедленно: без доказанного reference-aware Windows-safe reaper они остаются unreferenced и не возвращаются в live state;
- поэтому долгоживущая база после многих compactions может требовать больше disk space до появления safe obsolete-file cleanup;
- transactions, reverse iterator и compaction metrics ещё отсутствуют;
- range tombstones отсутствуют.

Подробнее: `docs/COMPACTION.md`, `docs/formats/MANIFEST_V2.md`, ADR 0010.
