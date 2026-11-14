# Инварианты ForgeDB

Статусы:

- **обеспечен** — код и автоматические тесты прошли обязательный release gate версии;
- **draft** — код и тесты добавлены в release-ветку, но финальный gate ещё не подтверждён;
- **частично** — обеспечена безопасная часть инварианта, но lifecycle ещё неполон;
- **запланирован** — feature нельзя заявлять до указанного milestone.

| ID | Инвариант | Статус | Доказательство |
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
| I-013 | Obsolete file не удаляется, пока используется Version/reader/compaction/checkpoint. | частично v0.6.1 | process-local best-effort cleanup после compaction и explicit retry отслеживает reader weak refs; restart-aware lifecycle не реализован |
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
| I-054 | Ошибка sync при close не переводит базу в Closed и не освобождает WAL/MANIFEST writers и native LOCK. | обеспечен v0.6.1 | deterministic WAL/MANIFEST sync injection, retry, reopen test; CI 42880868115 |
| I-055 | Публичный write при memtable limit либо полностью коммитит batch после одного flush/retry, либо не публикует его. | обеспечен v0.6.1 | `stability_v061` automatic flush and oversized atomic batch tests; CI 42880868115 |
| I-056 | После compaction физически удаляются только доказанно retired SST без живых TableReader. | обеспечен v0.6.1 | `manual_compaction_reclaims_unpinned_retired_sst_automatically`; active scan regression; CI 42880868115 |
| I-057 | SSTable open не материализует физические data blocks; random-access reads проверяют BlockHandle, trailer и CRC32C до возврата данных. | обеспечен v0.6.2 | `lazy_open_does_not_read_entire_sstable_and_cache_hits_avoid_io`, `lazy_data_block_checksum_is_verified_when_read_after_open`; CI 43548913230 |
| I-058 | Legacy default DB не мигрирует, а одноимённые user keys именованных CF изолированы самостоятельными WAL/LSM directories. | обеспечен v0.7.0 | `isolated_families_persist_same_keys_through_flush_and_reopen`; CI feature gate |
| I-059 | Active CF registry содержит только проверенные уникальные ID/names и публикуется после durable child initialization. | обеспечен v0.7.0 | registry CRC32C/malformed rejection; `corrupted_registry_fails_closed_without_silent_reset` |
| I-060 | Удалённые/неопознанные CF directories не подключаются и не удаляются по догадке; числовой ID не переиспользуется. | обеспечен v0.7.0 | `drop_is_logical_and_name_reuse_cannot_recover_old_data`, `orphan_directory_from_crash_is_not_reused_or_cleaned` |
| I-061 | Snapshot/transaction не смешивает состояния разных named CF, а read-only/open error не создаёт новую живую family. | обеспечен v0.7.0 | `transaction_snapshots_and_conflicts_are_scoped_to_one_family`, `readonly_rejects_mutations_and_missing_registered_family_is_corruption` |
| I-062 | Операции с CF registry и закрытие facade сериализованы; symlink/missing orphan path не превращается в live family. | обеспечен v0.7.0 | `column_families.rs` registry mutex + symlink metadata; integration reopen/drop/orphan tests |
| I-063 | Один FQL statement полностью проверен парсером до любого изменения данных, недопустимый хвост не исполняется. | обеспечен v0.8.0 | `fql_one_shot_checks_syntax_before_creating_database`, FQL parser unit tests |
| I-064 | EOF/EXIT/ошибка script не фиксирует открытый BEGIN, в REPL ошибка не применяет некорректную команду. | обеспечен v0.8.0 | `failed_script_rolls_back_open_transaction_and_stops_before_commit`, `repl_continues_after_invalid_statement_but_does_not_apply_it` |
| I-065 | FQL transaction и SCAN ограничены одной Column Family; read-only CLI не создаёт/изменяет persistent state. | обеспечен v0.8.0 | `changing_families_during_transaction_is_rejected_and_not_applied`, `scan_limit_is_respected_and_read_only_refuses_mutations` |
| I-066 | В point-read native SST pool одновременно resident не более table_cache_capacity файлов; при eviction повторное чтение проверяет SST identity и checksum. | обеспечен v0.8.1 | `point_read_budget_one_evicts_and_reopens_without_losing_values`, `native_handle_cache_reopens_with_bounded_fd_and_rechecks_header` |
| I-067 | Наличие pooled OS FD не разрешает premature SST deletion; при Windows cleanup дескриптор закрывается до удаления после освобождения всех TableReader references. | обеспечен v0.8.1 | `pinned_scan_survives_compaction_and_retirement_evicts_windows_handles` |
| I-068 | Snapshot-aware point reads и transaction conflict detection сохраняются после LRU evictions и reopen. | обеспечен v0.8.1 | `transaction_conflicts_and_snapshot_reads_survive_fd_eviction` |
| I-069 | Streaming SST cursor после проверки metadata/первого блока не хранит отдельный долгоживущий File; все последующие blocks используют общий resident handle LRU. | обеспечен v0.8.2 | `scan_many_sst_with_capacity_one_and_reopen_preserves_order_and_bytes`; `concurrent_get_and_streaming_scans_share_one_fd_budget` |
| I-070 | Compaction inputs используют тот же SST read pool, но старый iterator по-прежнему удерживает VersionSet и защищает физические SST от premature deletion. | обеспечен v0.8.2 | `compaction_inputs_use_bounded_pool_and_cleanup_preserves_old_scan` |
| I-071 | LRU budget ограничивает resident SST read pool одного Database, не кратковременные init/metadata открытые File и не все дескрипторы процесса. | обеспечен v0.8.2 | `table_handle_stats()`, streaming pool regression suite, ADR 0018 |
| I-072 | Native SST handle LRU index и resident File map согласованы на hit/miss/evict; выбор LRU не нарушает вместимость и правильный victim. | обеспечен v0.9.0 | `ordered_handle_lru_keeps_most_recent_file_and_maintains_exact_metrics` |
| I-073 | MVCC merge не меняет snapshot visibility, приоритет tombstone или обнаружение corrupt source при пропуске value clone. | обеспечен v0.9.0 | `mvcc_merge_discards_large_invisible_values_without_losing_snapshot_semantics`, iterator unit tests |
| I-074 | Conflict validation ищет максимальную committed sequence при перекрывающихся L0 и непересекающихся L1+ и не допускает stale transaction commit. | обеспечен v0.9.0 | `conflict_validation_binary_search_finds_versions_in_leveled_sst`, transaction regression suite |
| I-075 | Streaming scan сохраняет L0-newest-first и L1+ порядок, границы диапазона и lifetime VersionSet без полного clone списка SST TableHandle. | обеспечен v0.9.0 | `narrow_range_scan_preserves_order_across_many_sst_without_handle_vector`, streaming cursor regression suite |

## Правило изменения

Новый persisted-протокол или изменение write/recovery path обязаны обновить эту таблицу и добавить автоматическое доказательство соответствующего инварианта. Одной документации недостаточно для статуса **обеспечен**.

I-041–I-047 подтверждены проверками feature head `70bdad2586fb4f2cd9208f584fb5536a692186d1` (CI run `42780867814`: Ubuntu stable, Windows stable, Rust 1.89). Выпуск дополнительно проходит отдельный release gate. Для старых `draft v0.4.0` статус намеренно не повышен автоматически: требуется предметная проверка покрытия каждого инварианта. I-048–I-053 подтверждены тестами в CI run `42787135076` на feature head `f6e96f8de494fb15cd85c9e7894ec973eaf191be` (Ubuntu stable, Windows stable, Rust 1.89). Отдельный release workflow повторяет Linux gate. I-054–I-056 подтверждены feature head `bea0b52ed9ccabcd144b91ad55f7e6130453f2b8` (CI run `42880868115`: Ubuntu stable, Windows stable, Rust 1.89). I-057 подтверждён feature head `04bfd605b961d57447e3b9baed3b177228c60d8b` (CI run `43548913230`, Ubuntu stable, Windows stable и Rust 1.89). I-058–I-062 подтверждены feature head `2f53fbaf72558093870275e1353d27b69f7c97fd` (CI run `43552878243`: Ubuntu stable, Windows stable, Rust 1.89). I-063–I-065 подтверждены feature head `0d86143159ed35b07eaa43e2a789a6d2acefd9db` (CI run `43555440756`: Ubuntu stable, Windows stable, Rust 1.89). I-066–I-068 подтверждены feature head `d3a39c211efb1865fd8dd10b92bdfcb63e97d58e` (CI run `43558647759`: Ubuntu stable, Windows stable, Rust 1.89). I-069–I-071 подтверждены feature head `771f9a0feaf916e2ac9d53544364b60bf5bd6fb7` (CI `43560292687`: Ubuntu stable, Windows stable и Rust 1.89). Stress на 1024 SST помечен `#[ignore]` и стандартным CI не выполнялся. I-072–I-075 подтверждены feature head `78e7025c9807f3d9015819bb30721ab556e69c50` (CI `43562510705`: Ubuntu stable, Windows stable, Rust 1.89, полный Linux performance workload с реальными evictions). I-013 остаётся **частично**: process-local cleanup реализован, cross-restart garbage collection не заявляется.
