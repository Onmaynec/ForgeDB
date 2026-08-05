# Compaction ForgeDB

Документ описывает milestone `v0.4.0 — Leveled Compaction`.

## Текущее состояние

`v0.3.0` предоставляет immutable VersionSet, L0 newest-first reads, validated L1+ invariants, snapshots и bounded SST cursors. В `v0.4.0` уже добавлены deterministic picker, explicit MANIFEST V2 contract и bounded snapshot-aware merge. Physical installation L1+ пока не активирована.

## Цели v0.4.0

- deterministic compaction score;
- L0→L1 и Ln→Ln+1 input selection;
- bounded-memory merge;
- snapshot-aware MVCC retention;
- безопасные tombstone rules;
- target-sized output splitting;
- crash-safe installation;
- reference-aware obsolete file deletion;
- write slowdown/stop thresholds;
- manual compact API и CLI.

## Compaction score

### L0

```text
score = file_count / l0_compaction_trigger
```

L0 eligible, когда `file_count >= trigger`.

### L1+

```text
score = actual_level_bytes / target_level_bytes
```

```text
target(L1) = level1_target_size
target(Ln) = target(L1) * level_size_multiplier^(n-1)
```

Все вычисления используют checked arithmetic. Score сравнивается как rational number без floating-point rounding. При равном score выбирается меньший level.

## Input selection

### L0→L1

1. выбрать oldest L0 file;
2. расширить source set до closure по всем overlapping L0 files;
3. добавить все overlapping L1 files;
4. расширить range по L1 и повторять closure до стабильности;
5. если полный safe set превышает `max_compaction_bytes`, вернуть explicit blocker.

Нельзя исключить overlapping file только ради лимита: это нарушило бы L1 non-overlap invariant.

### Ln→Ln+1

1. выбрать deterministic source file из Ln;
2. добавить все overlapping files из Ln+1;
3. вычислить полный input range и bytes;
4. вернуть blocker при превышении configured limit.

Compaction pointer сохраняется в MANIFEST V2. Расширение source range будет добавлено до завершения milestone.

## Bounded merge

`CompactionMerge` объединяет sorted sources через binary heap.

Memory bound merge-слоя:

```text
O(number_of_sources + max_versions_per_user_key)
```

Каждый source предоставляет только current entry через `CompactionCursor`. Merge не materialize-ит весь input range.

Проверки:

- число sources ограничено до heap initialization;
- каждый source обязан выдавать internal keys строго по возрастанию;
- duplicate sequence одного logical key между sources является corruption;
- число versions одного logical key ограничено;
- checked counters не используют wrapping arithmetic;
- после первой ошибки iterator прекращает выдачу.

Logical key включает column family ID и user key.

## Snapshot retention

При отсутствии active snapshots:

- сохраняется только newest version;
- newest tombstone сохраняется, если output не bottommost;
- newest tombstone и все shadowed versions удаляются, если output bottommost.

При `oldest_active_snapshot = S`:

- сохраняются все versions с `sequence > S`;
- сохраняется первая version с `sequence <= S`;
- более старые shadowed versions удаляются;
- bottommost tombstone на snapshot floor можно удалить вместе со всеми более старыми versions, потому что ниже нет данных, которые могли бы воскреснуть.

Эта политика консервативно сохраняет visibility любых active snapshots с sequence не меньше `S`.

## Tombstone rules

Tombstone нельзя удалить только по возрасту. Для текущего merge slice удаление разрешено только когда:

1. output range доказан bottommost;
2. tombstone не новее oldest active snapshot либо active snapshots отсутствуют;
3. все более старые versions этого logical key одновременно исключаются из output.

Physical compaction picker обязан предоставить bottommost proof до включения этого режима.

## MANIFEST V2

MANIFEST V1 не изменяется и остаётся active production format до migration slice. V2 contract добавляет:

- exact level в add/delete records;
- authoritative level count;
- compaction pointers;
- L1+ non-overlap validation;
- minimum reader version 2.

Fixed V1 fixture проверяет, что добавление V2 не меняет V1 reader semantics.

## Installation protocol

Physical installation будет реализована только после V1→V2 migration:

1. построить outputs в `TEMP/`;
2. sync каждого output;
3. rename в canonical SST names;
4. sync database directory;
5. append один MANIFEST V2 edit: delete inputs + add outputs + compaction pointer;
6. sync MANIFEST;
7. publish new VersionSet;
8. пометить inputs obsolete;
9. удалить их только после освобождения active Version/readers.

## Failure policy

- picker и merge не изменяют filesystem;
- partial output не становится live;
- failure до MANIFEST sync оставляет outputs orphan/temp;
- failure после MANIFEST sync обязан восстанавливать новый VersionSet;
- background failure не скрывается от writes;
- unsafe partial plan не используется.

## Реализованные slices

### Picker

- no eligible level;
- score selection и tie-break;
- L0 overlap closure через L1;
- Ln overlap selection;
- explicit byte-limit blocker;
- target overflow rejection.

### MANIFEST V2 contract

- bounded header/frame codec;
- levelled add/delete;
- compaction pointers;
- state validation;
- V1 fixed fixture;
- future-version rejection.

### Merge

- heap merge;
- source ordering validation;
- duplicate sequence rejection;
- snapshot floor retention;
- bottommost tombstone dropping;
- bounded versions-per-key;
- metrics для input/output/dropped entries и tombstones.

Следующий slice: output splitting и sink protocol, затем V1→V2 migration и crash-safe physical installation.
