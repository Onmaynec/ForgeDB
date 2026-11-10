# Инварианты ForgeDB

Статусы:

- **обеспечен** — код и автоматические тесты прошли обязательный release gate версии;
- **draft** — код и тесты добавлены в release-ветку, но финальный gate ещё не подтверждён;
- **частично** — обеспечена безопасная часть инварианта, но lifecycle ещё неполон;
- **запланирован** — feature нельзя заявлять до указанного milestone.

| ID | Инвариант | Статус v0.6.0 | Доказательство |
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
| I-013 | Obsolete file не удаляется, пока используется Version/reader/compaction/checkpoint. | частично v0.6.0 | process-local manual cleanup отслеживает reader weak refs; полноценный restart-aware lifecycle не реализован |
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
| I-035 | Release gate использует зафиксированный dependency graph. | draft v0.4.0 | Cargo.lock; CI/release команды используют `--locked` |
| I-036 | Compaction installation является одним atomic VersionEdit относительно LSM topology. | draft v0.4.0 | один edit содержит delete L0/L1+ inputs и add target outputs; cloned-state validation перед publish |
| I-037 | Crash до compaction installation не делает output live; crash после synced installation не возвращает input в live state. | draft v0.4.0 | MANIFEST authoritative topology + reopen L0→L1 test; documented protocol |
| I-038 | MANIFEST V1 не изменяется in-place при upgrade до V2. | draft v0.4.0 | read-only V1 unchanged; writable rotation через новый MANIFEST + CURRENT replacement |
| I-039 | Output splitting не разделяет versions одного user key между SSTables. | draft v0.4.0 | `build_compaction_chunks` groups user key; dedicated unit test |
| I-040 | L0 stop backpressure не допускает unbounded growth через публичный write path. | draft v0.4.0 | stop threshold запускает synchronous compaction и затем возвращает `Busy`, если pressure остаётся |
| I-041 | Active transaction читает последнюю собственную buffered mutation раньше snapshot database state. | обеспечен v0.5.0 | `Transaction::own_write`; own-write/put-delete-read test |
| I-042 | Transaction commit отклоняется целиком, если любой written key имеет committed sequence выше snapshot sequence. | обеспечен v0.5.0 | mutable/immutable/SST validation; same-key, ABA sequence и flushed-SST conflict tests |
| I-043 | Между successful transaction conflict validation и WAL append не может вклиниться другой public writer. | обеспечен v0.5.0 | общий `StreamingDatabase::write_gate` охватывает ordinary writes и validate→write transaction path |
| I-044 | Successful transaction commit сохраняется одним существующим atomic WAL WriteBatch и не имеет частично восстанавливаемого transaction record. | обеспечен v0.5.0 | transaction reopen recovery test; WAL one-frame-per-batch invariant I-004 |
| I-045 | Active transaction удерживает зарегистрированный snapshot до commit/rollback/conflict/failure. | обеспечен v0.5.0 | `Transaction.snapshot`; terminal paths вызывают `take`; existing SnapshotRegistry oldest-sequence retention |
| I-046 | Transaction commit не принимает `DisableWal`. | обеспечен v0.5.0 | explicit validation + rejection/no-publication test + CLI integration test |
| I-047 | Transaction read set не выделяет память для invalid/oversized keys и ограничивает число и общую длину сохраняемых ключей. | обеспечен v0.5.0 | `read_set_rejects_oversized_keys_and_total_bytes_before_allocation` |
| I-048 | Snapshot-aware indexed SST lookup сохраняет tombstone и первую видимую версию при разрезании user key между blocks. | обеспечен v0.6.0 | SST unit test `snapshot_lookup_across_multiple_data_blocks_uses_cache`; integration `block_cache_preserves_snapshot_versions_across_sst_blocks` |
| I-049 | LRU-кэш хранит только validated decoded blocks и не превышает учтённый decoded-block budget. | обеспечен v0.6.0 | cache hit/eviction/disabled/failure unit tests; integration small-cache test |
| I-050 | Оптимистическая write-write conflict validation не пропускает newest sequence ключа после flush в SSTable даже при отключённом кэше. | обеспечен v0.6.0 | `cache_can_be_disabled_and_transaction_conflict_still_detected` |
| I-051 | Retired SST не удаляется до durable MANIFEST installation или при наличии живого TableReader. | обеспечен v0.6.0 | per-reader weak pin; `active_scan_defers_retired_sst_until_iterator_is_dropped`; synced edit queue |
| I-052 | Normal open/cleanup не удаляет неподтверждённый orphan SST после рестарта. | обеспечен v0.6.0 | `cleanup_never_guesses_origin_of_orphan_after_restart` |
| I-053 | Read snapshot сохраняет старую версию данных после compaction и физической очистки retired input. | обеспечен v0.6.0 | `registered_snapshot_still_reads_old_version_after_cleanup` |

## Правило изменения

Новый persisted-протокол или изменение write/recovery path обязаны обновить эту таблицу и добавить автоматическое доказательство соответствующего инварианта. Одной документации недостаточно для статуса **обеспечен**.

I-041–I-047 подтверждены проверками feature head `70bdad2586fb4f2cd9208f584fb5536a692186d1` (CI run `42780867814`: Ubuntu stable, Windows stable, Rust 1.89). Выпуск дополнительно проходит отдельный release gate. Для старых `draft v0.4.0` статус намеренно не повышен автоматически: требуется предметная проверка покрытия каждого инварианта. I-048–I-053 подтверждены тестами в CI run `42787135076` на feature head `f6e96f8de494fb15cd85c9e7894ec973eaf191be` (Ubuntu stable, Windows stable, Rust 1.89). Отдельный release workflow повторяет Linux gate. I-013 остаётся **частично**: process-local cleanup реализован, cross-restart garbage collection не заявляется.
