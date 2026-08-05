# Инварианты ForgeDB

Статусы:

- **обеспечен** — код и автоматические тесты прошли release gate выпущенной версии;
- **draft v0.4** — implementation и automated proof прошли feature-branch gate, но версия ещё не merged/released;
- **частично** — обеспечен только фундамент инварианта;
- **запланирован** — feature нельзя заявлять до указанного milestone.

| ID | Инвариант | Статус | Доказательство |
|---|---|---|---|
| I-001 | Один каталог базы одновременно открыт не более чем одним процессом ForgeDB. | обеспечен | native lock tests; lock удерживается до `close`/process exit |
| I-002 | Номер persisted-файла не переиспользуется после публикации или reservation в MANIFEST. | draft v0.4 | unified `next_file_number`; `max(manifest, WAL)+1`; orphan-aware allocator; compaction reservation tests |
| I-003 | Sequence number не равен нулю; persisted-диапазоны возрастают и не пересекаются. | обеспечен | primitive tests, WAL replay validation, DisableWal gap test |
| I-004 | Подтверждённый WAL WriteBatch при recovery либо применяется полностью, либо отсутствует полностью. | обеспечен | один frame на batch, full decode before apply, process crash tests |
| I-005 | Новое состояние становится видимым только после полного применения batch. | обеспечен | staged memtable apply под writer mutex |
| I-006 | MANIFEST не ссылается на SSTable, который не был полностью записан, синхронизирован и опубликован. | draft v0.4 | flush/compaction build→sync→rename→directory sync до final edit; missing-output/corruption tests |
| I-007 | SSTable неизменяем после installation. | обеспечен | immutable file lifecycle; checksum validation при open |
| I-008 | L1+ не содержит перекрывающихся file ranges. | draft v0.4 | `ManifestStateV2` и immutable `VersionSet` validation; overlapping replacement rejection |
| I-009 | L0 point lookup проверяет files от новых к старым. | обеспечен | newest-first read order; overwrite-after-flush tests |
| I-010 | Snapshot не наблюдает sequence выше собственного. | обеспечен v0.3 | snapshot model, overwrite/flush tests |
| I-011 | Compaction не удаляет version, которая может быть видима active snapshot. | draft v0.4 | oldest-snapshot merge retention; physical compaction snapshot/reopen test |
| I-012 | Tombstone удаляется только при snapshot-safe bottommost proof. | draft v0.4 | bottommost/non-bottommost merge tests; dropped-tombstone metrics regression |
| I-013 | Obsolete SST сохраняется, пока его использует active Version или streaming reader. | draft v0.4 | `Weak<VersionSet>` queue; cursor `Arc` lease; active-scan deletion deferral test |
| I-014 | Каждая persisted length и offset проверяется до allocation или I/O. | draft v0.4 | bounded WAL/SST/MANIFEST V1/V2 decoders; cursor/merge/output limits |
| I-015 | Неизвестная future persisted version не открывается как текущая. | draft v0.4 | V1/V2 exact dispatch и future-header/frame rejection tests |
| I-016 | Verify и file inventory не изменяют базу. | draft v0.4 | read-only inventory regression; verify path не вызывает cleanup |
| I-017 | Salvage никогда не изменяет исходную базу. | запланирован v0.9.0 | salvage integration tests |
| I-018 | Normal recovery не продолжается после mid-file corruption. | обеспечен для WAL/SSTable/MANIFEST V1; draft для V2 | checksum/corruption tests; отсутствие resync; bounded V2 replay |
| I-019 | DatabaseId каждого live storage file совпадает с `IDENTITY`. | draft v0.4 | WAL/SST/MANIFEST V1/V2 header validation и mismatch tests |
| I-020 | Ошибка storage write видима последующим writes. | обеспечен для WAL/MANIFEST/flush | fatal writer state после append/sync error |
| I-021 | Только последний физически неполный WAL/MANIFEST frame может быть отброшен согласно format policy. | draft v0.4 | trailing partial WAL и MANIFEST V2 replay tests |
| I-022 | Projected memtable memory проверяется до WAL mutation. | обеспечен | preflight accounting tests |
| I-023 | `Sync` выполняет WAL sync до memtable visibility. | обеспечен | write order и process-abort test |
| I-024 | `close()` освобождает writers и системную блокировку только после успешного sync. | draft v0.4 | close/reopen tests; WAL/MANIFEST sync до lock release |
| I-025 | После WAL rotation previous и current поколения восстанавливаются в разные memtable. | обеспечен v0.2 | rotated-recovery test; mutable → immutable → L0 read order |
| I-026 | MANIFEST-owned current/previous WAL не пересоздаётся и не игнорируется при отсутствии. | обеспечен v0.3 | missing-active-WAL unit/integration regression tests |
| I-027 | Старый WAL удаляется только после успешного MANIFEST installation edit и sync. | обеспечен v0.2 | flush protocol; reopen-without-obsolete-WAL test |
| I-028 | Повторный flush использует то же reserved SST generation и удаляет только доказанный orphan этого номера. | обеспечен v0.2 | generation validation; deterministic orphan cleanup |
| I-029 | Параллельные flush не строят два SSTable для одной immutable memtable. | обеспечен v0.2 | `flush_in_progress`; `Busy` для второго flush и close |
| I-030 | Перед WAL rotation текущее WAL-поколение синхронизируется. | обеспечен v0.2 | explicit WAL sync до rotation edit |
| I-031 | Initial WAL создан и синхронизирован до публикации `CURRENT`. | обеспечен v0.2 | WAL-before-CURRENT bootstrap tests |
| I-032 | Header-only orphan WAL можно повторно использовать только после проверки UUID, file number и отсутствия frame/tail. | обеспечен v0.2 | strict idempotent `create_wal`; orphan tests |
| I-033 | Bootstrap cleanup удаляет только доказанные reserved artifacts; unknown/user files сохраняются. | обеспечен v0.2 | bootstrap orphan cleanup test |
| I-034 | Существующий `CURRENT` запрещает повторную initial initialization. | обеспечен v0.2 | existing-current rejection test |
| I-035 | Release gate использует зафиксированный dependency graph. | draft v0.4 | `Cargo.lock`; все CI commands используют `--locked` |
| I-036 | Current VersionSet меняется только после успешного sync final compaction MANIFEST edit. | draft v0.4 | installation order; `after_compaction_manifest_sync` process-abort/reopen test |
| I-037 | Crash до final compaction edit не делает output live; crash после sync edit восстанавливает new Version. | draft v0.4 | child-process matrix `after_output_sync`/`after_manifest_sync` на Ubuntu и Windows |
| I-038 | Compaction удаляет только exact selected inputs и устанавливает outputs в непосредственный следующий level. | draft v0.4 | immutable replacement ownership/level tests; missing input/duplicate rejection |
| I-039 | Manual range compaction выбирает пересекающий source deterministic и не обходится pressure из другого range. | draft v0.4 | manual picker range/starvation tests |
| I-040 | Public manual compact имеет progress proof и hard bound; бесконечный installation loop запрещён. | draft v0.4 | original-source work set, before/after source layout, 10 000-step limit test |
| I-041 | L0 stop threshold блокирует writes, но не reads или manual compaction. | draft v0.4 | write-stall integration test и threshold unit tests |
| I-042 | Delay write slowdown выполняется вне database state lock и ограничен 50 ms. | draft v0.4 | deterministic stall decision tests; lock boundary в write path |
| I-043 | `file_inventory()` не является cleanup trigger. | draft v0.4 | iterator-drop + repeated inventory + later write cleanup regression |

## Правило изменения

Новый persisted protocol или изменение write/recovery path обязаны обновить эту таблицу и добавить автоматическое доказательство соответствующего инварианта. Одной документации недостаточно для статуса **обеспечен**.

Статус **draft v0.4** переводится в **обеспечен** только после merge, release gate и публикации версии.
