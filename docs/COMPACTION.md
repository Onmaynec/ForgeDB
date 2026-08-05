# Compaction ForgeDB

Документ описывает development milestone `v0.4.0 — Leveled Compaction`. До merge/release это draft contract, а не гарантия выпущенной версии.

## Текущее состояние

ForgeDB физически использует levels L0…Ln, MANIFEST V2 и immutable `VersionSet`. Реализованы:

- deterministic background score picker;
- deterministic manual range picker;
- L0→L1 и Ln→Ln+1 selection;
- bounded snapshot-aware merge;
- bottommost tombstone rules;
- streaming output splitting;
- crash-safe physical installation;
- reference-aware obsolete SST lifetime;
- bounded L0 slowdown/stop policy;
- public bounded exhaustive `Database::compact_range`;
- process-crash matrix installation boundaries.

Compaction пока синхронна. Полноценный background scheduler и stall metrics не включены в текущий production path.

## Compaction score

### L0

```text
score = file_count / l0_compaction_trigger
```

### L1+

```text
score = actual_level_bytes / target_level_bytes

target(L1) = level1_target_size
target(Ln) = target(L1) * level_size_multiplier^(n-1)
```

Score хранится как rational number без floating-point rounding. Arithmetic checked. При равном score выбирается меньший level.

## Picker-ы

### Background picker

Выбирает highest eligible score. Он остаётся чистой функцией и не изменяет filesystem.

### Manual range picker

Manual picker не требует `score >= 1`. Он выбирает первый deterministic source SST, пересекающий half-open range `[start, end)`, поэтому pressure в другом диапазоне не вытесняет requested range.

Empty range возвращает отсутствие plan. Invalid `start > end` отклоняется.

### L0→L1

1. выбрать seed L0 file;
2. расширить L0 set до overlap closure;
3. добавить overlapping L1 files;
4. расширять общий range, пока closure не стабилизируется;
5. вернуть blocker, если полный safe set превышает `max_compaction_bytes`.

Overlapping input нельзя исключить ради лимита: это нарушило бы L1 non-overlap invariant.

### Ln→Ln+1

1. выбрать deterministic source file из Ln;
2. добавить все overlaps из Ln+1;
3. вычислить полный input range/bytes;
4. применить `max_compaction_bytes`.

## Bounded merge

Input SST читаются `SstEntryCursor` по одному data block на source. `CompactionMerge` объединяет current entries через binary heap.

Memory bound merge-слоя:

```text
O(number_of_sources + max_versions_per_user_key)
```

Проверяются:

- maximum source count;
- strict internal-key order каждого source;
- duplicate sequence одного logical key;
- maximum versions одного key;
- checked counters;
- single-error termination.

## Snapshot retention

При `oldest_active_snapshot = S`:

- сохраняются все versions с `sequence > S`;
- сохраняется первая version с `sequence <= S`;
- более старые shadowed versions можно удалить.

При отсутствии active snapshots сохраняется только newest необходимая version.

## Tombstone rules

Tombstone удаляется только когда одновременно доказано:

1. output range bottommost;
2. tombstone не новее oldest active snapshot либо snapshots отсутствуют;
3. все более старые versions logical key удаляются вместе с ним;
4. ниже output level нет file, способного воскресить value.

Возраст tombstone сам по себе не является достаточным условием.

## Output splitting

Splitter выдаёт protocol:

```text
BeginFile → Entry* → FinishFile
```

Split выполняется только между logical-key groups. Versions одного key не делятся между outputs. Oversized group остаётся в одном output и ограничивается отдельными hard limits.

## Installation protocol

Один installation step выполняется так:

1. открыть exact selected inputs с checksum verification;
2. выполнить bounded merge и output splitting;
3. durably зарезервировать каждый output file number в MANIFEST V2;
4. построить `TEMP/SST-{number}.tmp`;
5. sync output;
6. rename в canonical SST name;
7. sync database directory;
8. подготовить immutable replacement `VersionSet` и повторно проверить ownership/ranges/L1+ overlap;
9. append один final MANIFEST V2 edit: delete inputs + add outputs + compaction pointer;
10. sync MANIFEST;
11. publish new in-memory Version;
12. поставить input SST в reference-aware obsolete queue;
13. удалить input только после освобождения старой Version.

Current Version никогда не меняется до успешного MANIFEST sync.

## Process-crash outcomes

Test-only failpoints существуют только в unit-test build.

### После output sync, до final MANIFEST edit

Recovery использует старый VersionSet. Inputs остаются live. Полностью построенный output классифицируется orphan и не включается автоматически.

### После final MANIFEST sync, до Version swap

Recovery использует новый VersionSet из MANIFEST. Output live, inputs orphan/obsolete. In-memory swap потерян вместе с process и не является authoritative.

### Перед obsolete input delete

Persisted outcome тот же: output live, inputs безопасно остаются orphan после crash.

Child-process matrix проверяет latest values и inventory на Ubuntu/Windows. Это доказательство process crash/forced termination, но не OS crash или power loss.

## Obsolete lifetime

Streaming cursors удерживают `Arc<VersionSet>`. Queue хранит `Weak<VersionSet>` и exact file numbers. Пока weak reference можно upgrade, physical deletion запрещён.

`file_inventory()` и verify не выполняют cleanup. Retry cleanup запускают только mutating paths: write, flush, compaction и close.

Crash до cleanup может оставить безопасный orphan SST. Cleanup intent пока не persisted.

## Manual compact

Public `compact_range`:

1. валидирует bounds;
2. flush-ит mutable state;
3. фиксирует set исходных source SST диапазона;
4. повторяет полный installation step, пока все исходные file numbers не consumed;
5. проверяет source-layout progress после каждого step;
6. ограничивает вызов 10 000 steps.

Newly created outputs не добавляются в исходный work set. Следующий manual вызов может продвинуть их дальше.

## Write stalls

- ниже slowdown trigger: без задержки;
- от slowdown trigger до stop trigger: delay по 2 ms за pressure step, максимум 50 ms;
- на stop trigger: write возвращает `Error::Busy`.

Sleep выполняется вне database state lock. После delay threshold проверяется повторно. Reads и manual compaction остаются доступны.

## Известные ограничения

- compaction синхронна и удерживает database state lock во время I/O;
- нет background scheduler и compaction/stall metrics;
- delay не interruptible в пределах hard cap 50 ms;
- cleanup intent не переживает process crash;
- disk-full, injected sync/rename failures, OS-crash и power-loss matrix относятся к hardening milestone.
