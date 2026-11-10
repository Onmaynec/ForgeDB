# Leveled compaction ForgeDB v0.6.1

## Scope

ForgeDB v0.4.0 вводит synchronous leveled compaction поверх immutable SSTable V1. L0 допускает overlap; L1+ обязаны быть отсортированы по user-key range и не перекрываться внутри уровня.

Compaction является изменением структуры LSM, но не формата SSTable: output tables остаются SSTable V1. Persisted placement L1+ хранится в MANIFEST V2.

## Picker

Default thresholds:

- L0 compaction trigger: 4 files;
- L0 slowdown trigger: 8 files;
- L0 stop trigger: 12 files;
- L1 target: 256 MiB;
- level multiplier: 10;
- target SSTable size: 64 MiB;
- maximum compaction resident/input budget: `max_compaction_bytes`.

### L0 → L1

Picker выбирает L0 seed и расширяет L0 input до closure по пересекающимся user-key ranges. Затем добавляет все L1 files, пересекающие итоговый диапазон.

### Ln → Ln+1

Для L1+ compaction eligible, когда фактический размер уровня достигает его target. Выбирается один source file и все пересекающиеся files следующего уровня.

Manual `compact_range(start, end)` выполняет flush и принудительно продвигает пересекающиеся files вниз до отсутствия дальнейшей eligible compaction. Bounds являются half-open `[start, end)`.

## Merge и memory bound

Inputs читаются через проверенный SSTable path. Все bytes compaction учитываются checked arithmetic и ограничиваются `max_compaction_bytes`. Это bounded, но v0.4.0 ещё не является fully streaming merge: выбранный job materialize-ит entries до установленного budget.

Internal entries сортируются canonical `InternalKey` comparator:

1. column family ascending;
2. user key ascending;
3. sequence descending;
4. value type descending.

Duplicate exact internal key между inputs считается corruption.

Outputs делятся около `target_sst_size`, но никогда не разрывают версии одного user key между двумя output SSTables.

## MVCC retention

Для каждого user key compaction всегда сохраняет newest version, если она не является безопасно удаляемым bottommost tombstone.

При active snapshots используется `oldest_active_snapshot_sequence`:

- сохраняются все версии новее oldest snapshot;
- сохраняется первая версия с `sequence <= oldest snapshot`;
- более старые shadowed versions могут быть отброшены.

Этого достаточно, чтобы любой активный snapshot сохранил прежний observable result.

## Tombstones

Tombstone удаляется только когда:

- compaction range bottommost: ниже output level нет overlapping files;
- newest entry для ключа является tombstone;
- нет active snapshot, которому требуется состояние до tombstone, либо tombstone sequence не новее oldest active snapshot.

При этих условиях вместе с tombstone удаляются и закрытые им older versions из compaction inputs.

## Installation protocol

Compaction использует два durable MANIFEST boundary.

### 1. Reservation

После формирования output chunks ForgeDB резервирует непрерывный диапазон file numbers через MANIFEST edit, который продвигает `next_file_number` и синхронизируется до создания output SSTables.

Crash после reservation не переиспользует номера.

### 2. Output construction

Каждый output создаётся обычным SSTable builder:

1. TEMP file;
2. complete write;
3. file sync;
4. close;
5. atomic rename в `SST-*.sst`;
6. directory sync.

### 3. Atomic installation

Один MANIFEST V2 edit одновременно:

- удаляет все input file references;
- добавляет output references в target level;
- сохраняет last sequence.

Только после successful MANIFEST sync новый immutable `VersionSet` становится current.

Crash до installation оставляет старые inputs authoritative. Crash после installation делает outputs authoritative.

## Obsolete physical files

В v0.6.0 input SST включаются в process-local retirement queue только после durable MANIFEST compaction installation edit. Каждый retired file хранит weak reference на исходный `TableReader`, который может использоваться многими версиями LSM и активными итераторами. `cleanup_obsolete_sstables()` удаляет только proven retired files после освобождения reader и отсутствия номера в live VersionSet. CLI `compact` вызывает эту проверку после compaction.

С v0.6.1 после compaction выполняется best-effort cleanup. Уже подтверждённая compaction не меняет статус из-за ошибки unlink; явный cleanup возвращает подробный отчёт и позволяет retry после освобождения читателей. Это всё ещё не full cross-restart garbage collection: после нового открытия неподтверждённые orphan-файлы не удаляются. Подробности: [CACHE_AND_PERFORMANCE.md](CACHE_AND_PERFORMANCE.md) и ADR 0013.

## Write backpressure

Перед публичными writes проверяется L0 file count.

- при slowdown threshold выполняется bounded sleep вне database state lock;
- при stop threshold сначала запускается synchronous compaction;
- если pressure не удалось снизить, write возвращает `Busy` вместо unbounded accumulation.

Reads продолжают использовать immutable VersionSet.

## Error behavior

- malformed/overlapping L1+ metadata блокирует open как corruption;
- oversized compaction job возвращает `ResourceLimit`;
- изменение input Version до installation возвращает `Busy`;
- MANIFEST failure переводит writer path в existing fatal/background storage error state;
- unknown future MANIFEST version отклоняется.

## Known limitations

- compaction synchronous, без background worker pool;
- merge bounded, но materialized внутри `max_compaction_bytes`;
- process-local best-effort cleanup после compaction есть; restart-aware garbage collection ещё нет;
- compaction metrics/statistics появятся вместе с observability milestone;
- range tombstones отсутствуют.
