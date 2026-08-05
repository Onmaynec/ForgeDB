# ADR-0007: Leveled compaction и versioned installation

Статус: принят для milestone `v0.4.0`.

## Контекст

ForgeDB `v0.3.0` умеет читать L0 и validated модель L1+, но MANIFEST V1 хранит только L0 metadata. Добавление physical L1+ files напрямую в V1 нарушило бы stable persisted contract.

## Решение

Compaction реализуется вертикальными slices:

1. deterministic score и input picker без filesystem mutation;
2. MANIFEST V2 с явным level для added/deleted files;
3. V1 compatibility reader и V2 writer;
4. bounded merge и output splitting;
5. crash-safe installation одним VersionEdit;
6. obsolete-file lifetime и write stalls;
7. manual `compact_range`.

Picker использует:

- L0 score: `file_count / l0_compaction_trigger`;
- L1+ score: `actual_level_bytes / target_level_bytes`;
- highest eligible score;
- tie-break в пользу меньшего level;
- L0→L1 overlap closure по L0 и L1;
- Ln→Ln+1: deterministic source file и все overlapping target files;
- explicit blocker, если полный безопасный input set превышает `max_compaction_bytes`.

## Persisted-format policy

- MANIFEST V1 не изменяется;
- physical L1+ installation запрещена до MANIFEST V2;
- V2 должен иметь отдельный format version, fixtures V1, future-version rejection и compatibility matrix;
- compaction outputs не считаются live до durable MANIFEST V2 edit;
- input files не удаляются до sync edit и освобождения active Version handles.

## Инварианты

- picker детерминирован для одного VersionSet и Options;
- L0 closure нельзя обрезать ради byte limit;
- превышение лимита возвращает blocker, а не unsafe partial plan;
- L1+ output ranges должны быть непересекающимися;
- compaction не удаляет version, видимую oldest active snapshot;
- tombstone удаляется только при snapshot-safe и bottommost доказательстве;
- persisted V1 не переписывается как V2 молча.

## Последствия

Первый slice не уменьшает L0 и не изменяет observable state. Он создаёт проверяемую основу для выбора inputs. Запись уровней и физическая compaction появятся только после отдельного compatibility slice MANIFEST V2.
