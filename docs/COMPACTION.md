# Compaction ForgeDB

Документ описывает milestone `v0.4.0 — Leveled Compaction`.

## Текущее состояние

`v0.3.0` предоставляет immutable VersionSet, L0 newest-first reads, validated L1+ invariants, snapshots и bounded SST cursors. Физические L1+ files пока отсутствуют. MANIFEST V1 хранит только L0 metadata.

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

Compaction pointer и расширение source range будут добавлены до завершения milestone.

## Merge retention

Для каждого user key output merge обязан:

- сохранить newest version;
- сохранить versions, которые могут быть видимы active snapshots;
- не удалять tombstone без bottommost proof;
- не разделять versions одного user key между outputs некорректно;
- сохранять internal-key ordering.

## Installation protocol

MANIFEST V1 не поддерживает levels. Physical installation будет реализована только после MANIFEST V2:

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

- picker не изменяет filesystem;
- partial output не становится live;
- failure до MANIFEST sync оставляет outputs orphan/temp;
- failure после MANIFEST sync обязан восстанавливать новый VersionSet;
- background failure не скрывается от writes;
- unsafe partial plan не используется.

## Первый slice

Crate `forgedb-lsm` реализует pure deterministic picker и tests:

- no eligible level;
- score selection и tie-break;
- L0 overlap closure через L1;
- Ln overlap selection;
- explicit byte-limit blocker;
- target overflow rejection.

Следующий slice: MANIFEST V2 design, encoder/decoder, V1 fixtures и compatibility tests.
