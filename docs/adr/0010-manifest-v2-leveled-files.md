# ADR 0010: MANIFEST V2 для persisted L1+ placement

## Status

Accepted for ForgeDB v0.4.0.

## Context

ForgeDB v0.3.0 уже имела in-memory `VersionSet` с L1+ invariants, но MANIFEST V1 сохранял только L0 `ADD_FILE`/`DELETE_FILE`. Реализация leveled compaction без изменения MANIFEST могла бы работать только до закрытия процесса: после reopen target level output потерял бы placement и база либо считала бы файл orphan, либо была бы вынуждена угадывать уровень. Оба варианта нарушают stable persisted contracts и crash recovery.

## Decision

Ввести MANIFEST V2.

V2 сохраняет V1 header/frame structure, но повышает format version и добавляет два versioned fields:

- `ADD_LEVEL_FILE(level, TableMeta)`;
- `DELETE_LEVEL_FILE(level, file_number)`.

L0 fields V1 остаются отдельными и сохраняют старое encoding.

ForgeDB v0.4 читает V1 и V2. Read-only open V1 ничего не изменяет. Read-write open V1 crash-safe ротирует полный validated snapshot в новый V2 MANIFEST и публикует его через `CURRENT`.

## Invariants

- нельзя реконструировать level placement из имени файла или диапазона ключей;
- MANIFEST остаётся authoritative источником live LSM topology;
- compaction delete/add публикуются одним checksummed VersionEdit frame;
- V1 reader обязан отвергнуть V2 как future format;
- V2 reader обязан отвергнуть unknown future version/tag;
- migration не перезаписывает V1 in-place;
- новый MANIFEST получает уникальный file number и увеличивает `next_file_number`.

## Consequences

Плюсы:

- L1+ переживает close/reopen и crash;
- installation compaction имеет один durable commit point;
- V1 базы можно открыть без destructive migration;
- old MANIFEST остаётся доступным до durable CURRENT replacement.

Минусы:

- ForgeDB v0.3 не может открыть базу после writable upgrade до V2;
- compatibility matrix теперь должна различать read V1/V2 и write V2;
- тесты recovery обязаны учитывать orphan new MANIFEST до CURRENT replacement.

## Rejected alternatives

### Хранить все outputs как L0

Отвергнуто: это не leveled compaction и не обеспечивает L1+ non-overlap/read-path semantics.

### Вычислять level при reopen

Отвергнуто: placement является частью authoritative LSM state; эвристическое восстановление может изменить read order и observable state.

### Изменить V1 без version bump

Отвергнуто как нарушение persisted contract policy.
