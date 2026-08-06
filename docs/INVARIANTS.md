# Инварианты ForgeDB

Статусы:

- **обеспечен** — код и автоматические тесты прошли обязательный release gate версии;
- **draft** — код и тесты добавлены в release-ветку, но финальный gate ещё не подтверждён;
- **частично** — обеспечена безопасная часть инварианта, но lifecycle ещё неполон;
- **запланирован** — feature нельзя заявлять до указанного milestone.

| ID | Инвариант | Статус перед v0.4.0 gate | Доказательство |
|---|---|---|---|
| I-001 | Один каталог базы одновременно открыт не более чем одним процессом ForgeDB. | обеспечен | native lock tests; lock удерживается до close/drop/process exit |
| I-002 | Номер persisted-файла не переиспользуется. | draft v0.4.0 | `next_file_number`; MANIFEST validation; flush reservation; compaction reservation edit до output creation |
| I-003 | Sequence number никогда не уменьшается и не повторяется. | обеспечен | primitive tests; WAL replay; MANIFEST last-sequence validation |
| I-004 | Подтверждённый WAL WriteBatch при recovery либо применяется полностью, либо отсутствует полностью. | обеспечен | один frame на batch; full decode before apply; crash tests |
| I-005 | Visible sequence публикуется только после полного применения batch. | обеспечен | staged memtable apply под writer serialization |
| I-006 | MANIFEST не ссылается на SSTable, не прошедший build/sync/rename protocol. | draft v0.4.0 | flush и compaction строят outputs до installation edit |
| I-007 | SSTable immutable после installation. | обеспечен | immutable file lifecycle; reader validation |
| I-008 | L1+ не содержат перекрывающихся file ranges внутри уровня. | draft v0.4.0 | `ManifestState::validate`; `VersionSet::validate`; overlap tests; compaction output install validation |
| I-009 | L0 point lookup проверяет files от новых к старым. | обеспечен | `level_zero.iter().rev()`; overwrite/flush tests |
| I-010 | Snapshot никогда не видит sequence выше собственного snapshot sequence. | обеспечен v0.3.0 | snapshot/read model tests |
| I-011 | Compaction не удаляет version, которая может быть видима active snapshot. | draft v0.4.0 | `retain_versions`; oldest-snapshot boundary unit test |
| I-012 | Tombstone удаляется только при доказанной безопасности. | draft v0.4.0 | bottommost + oldest snapshot condition; tombstone unit test |
| I-013 | Obsolete file не удаляется, пока используется Version/reader/compaction/checkpoint. | частично v0.4.0 | v0.4 консервативно не удаляет physical compaction inputs вообще; reference-aware reaper ещё не реализован |
| I-014 | Persisted length/offset проверяется до allocation/read. | draft для MANIFEST V2, обеспечен для V1 | bounded codec, checked arithmetic, max batch/key limits |
| I-015 | Неизвестная future persisted version не открывается как текущая. | draft для MANIFEST V2, обеспечен для V1 | MANIFEST accepts only 1..=2; unknown tags/versions rejected |
| I-016 | Verify не изменяет базу. | обеспечен v0.3.0 | verify/inventory tests |
| I-017 | Salvage не изменяет исходную базу. | запланирован v0.9.0 | salvage integration tests |
| I-018 | Normal recovery не продолжает replay после mid-file corruption. | draft для MANIFEST V2, обеспечен для WAL/SST/MANIFEST V1 | checksums; no resync; strict edit replay |
| I-019 | Database UUID live WAL/SST/MANIFEST соответствует IDENTITY. | обеспечен | header validation и mismatch tests |
| I-020 | Background/storage write failure не скрывается от последующих writes. | обеспечен для существующего synchronous path | fatal writer state после WAL/MANIFEST append/sync error |
| I-021 | Только последний физически неполный WAL frame может быть отброшен. | обеспечен | trailing partial tests |
| I-022 | Projected memtable memory проверяется до WAL mutation. | обеспечен | preflight accounting tests |
| I-023 | `Sync` выполняет WAL sync до memtable visibility. | обеспечен | write ordering и process-abort test |
| I-024 | `close()` освобождает WAL writer и process lock только после successful sync. | обеспечен | close/reopen tests |
| I-025 | После WAL rotation previous/current поколения восстанавливаются раздельно. | обеспечен | rotated recovery test |
| I-026 | MANIFEST-owned current/previous WAL не пересоздаётся и не игнорируется при отсутствии. | обеспечен | missing-active-WAL regression tests |
| I-027 | Старый WAL удаляется только после successful MANIFEST installation edit. | обеспечен | flush protocol; reopen test |
| I-028 | Повторный flush использует то же reserved SST generation и удаляет только доказанный orphan этого номера. | обеспечен | generation validation; deterministic orphan cleanup |
| I-029 | Параллельные maintenance jobs не строят competing outputs для одной state generation. | draft v0.4.0 | shared `flush_in_progress` сериализует flush/compaction и блокирует close |
| I-030 | Перед WAL rotation текущее WAL поколение синхронизируется. | обеспечен | explicit sync до rotation edit |
| I-031 | Initial WAL создан и синхронизирован до публикации CURRENT. | обеспечен | MANIFEST bootstrap tests |
| I-032 | Header-only orphan WAL переиспользуется только после strict validation. | обеспечен | idempotent create WAL tests |
| I-033 | Без CURRENT bootstrap cleanup удаляет только reserved bootstrap artifacts. | обеспечен | bootstrap cleanup test |
| I-034 | Existing CURRENT запрещает повторную initial MANIFEST initialization. | обеспечен | existing-current rejection test |
| I-035 | Release gate использует зафиксированный dependency graph. | draft v0.4.0 | Cargo.lock 0.4.0; CI/release команды используют `--locked` |
| I-036 | Compaction installation является одним atomic VersionEdit относительно LSM topology. | draft v0.4.0 | один edit содержит delete L0/L1+ inputs и add target outputs; cloned-state validation перед publish |
| I-037 | Crash до compaction installation не делает output live; crash после synced installation не возвращает input в live state. | draft v0.4.0 | MANIFEST authoritative topology + reopen L0→L1 test; documented protocol |
| I-038 | MANIFEST V1 не изменяется in-place при upgrade до V2. | draft v0.4.0 | read-only V1 unchanged; writable rotation через новый MANIFEST + CURRENT replacement |
| I-039 | Output splitting не разделяет versions одного user key между SSTables. | draft v0.4.0 | `build_compaction_chunks` groups user key; dedicated unit test |
| I-040 | L0 stop backpressure не допускает unbounded growth через публичный write path. | draft v0.4.0 | stop threshold запускает synchronous compaction и затем возвращает `Busy`, если pressure остаётся |

## Правило изменения

Новый persisted-протокол или изменение write/recovery path обязаны обновить эту таблицу и добавить автоматическое доказательство соответствующего инварианта. Одной документации недостаточно для статуса **обеспечен**.

После successful v0.4.0 release gate строки со статусом `draft v0.4.0` могут быть переведены в **обеспечен** только там, где фактические tests покрывают заявленный invariant. I-013 остаётся **частично**, пока не появится reference-aware obsolete-file reaper.
