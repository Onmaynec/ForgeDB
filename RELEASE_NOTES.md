# ForgeDB v0.3.0 — Full LSM Read Path

Статус выпуска: GitHub Pre-release.

ForgeDB получила полный snapshot-aware read path поверх mutable и immutable memtables, L0 и модели L1+. Версия добавляет bounded-memory range scan, MVCC snapshots, immutable VersionSet, file inventory, recovery planning и deep verification без изменения persisted-форматов V1.

## Основные изменения

- MVCC memtable с сохранением нескольких internal-key versions;
- cheap snapshots и registry активных sequence numbers;
- `ReadOptions` со snapshot и checksum policy;
- snapshot-aware point lookup;
- immutable `VersionSet`;
- L0 lookup от новых файлов к старым;
- validated L1+ model с sorted/non-overlap invariant;
- forward merge iterator на binary heap;
- block-at-a-time SSTable cursor;
- bounded-memory public range scan с bounds и limit;
- file inventory с live, obsolete, orphan, temporary, backup, user и unknown категориями;
- неразрушающий `RecoveryPlan::inspect`;
- quick и deep verify;
- CLI `scan`, `inventory`, `verify`, `verify --deep`, `inspect recovery-plan`.

## Read path

Для snapshot sequence чтение выполняется в порядке:

1. mutable memtable;
2. immutable memtable;
3. L0 SSTables newest-first;
4. не более одного candidate file на каждый L1+ level.

Range iterator объединяет источники через heap, скрывает старые versions и tombstones, учитывает snapshot и держит в памяти только текущий data block каждого SST cursor, а не весь диапазон.

## Snapshots и MVCC

- snapshot фиксирует visible sequence;
- cloned snapshot использует одну registry lease;
- snapshot другой database handle отклоняется;
- snapshot продолжает видеть старую value после overwrite и flush;
- registry предоставляет oldest active snapshot для будущих compaction retention rules.

## Recovery и corruption policy

- все current и previous WAL, на которые ссылается MANIFEST, проверяются до replay или append-open;
- отсутствующий MANIFEST-owned WAL является corruption и не пересоздаётся пустым;
- recovery plan только анализирует каталог и не изменяет файлы;
- unknown, backup и user files не попадают в automatic cleanup;
- deep verify проверяет SST block checksums, ordering, metadata ranges и level invariants;
- normal recovery не включает orphan SST в live state.

## Persisted-форматы

| Формат | Reader | Writer |
|---|---:|---:|
| IDENTITY | 1 | 1 |
| Internal Key | 1 | 1 |
| WAL | 1 | 1 |
| SSTable | 1 | 1 |
| MANIFEST | 1 | 1 |
| CURRENT | 1 | 1 |

Версия `v0.3.0` не добавляет новый persisted format и сохраняет совместимость с V1-контрактами `v0.2.0`.

## Проверка

Feature candidate на head `620a90e9bd697c63efe92b4fee6c65f8636bf571` прошёл GitHub Actions run `31009363168`:

- Ubuntu stable: canonical rustfmt, strict Clippy `-D warnings`, workspace tests, doctests и release build;
- Windows stable: тот же полный набор;
- Ubuntu Rust 1.89: strict Clippy и workspace tests.

Release PR повторно проходит ту же cross-platform matrix. После merge release workflow ещё раз выполняет полный Linux gate до создания annotated tag и GitHub Pre-release.

## Ограничения

- физические L1+ файлы ещё не создаются: leveled compaction входит в `v0.4.0`;
- reverse iterator отсутствует;
- transactions отсутствуют;
- flush остаётся синхронным;
- write stalls и background compaction ещё не реализованы;
- normal recovery не удаляет произвольные orphan/unknown files.
