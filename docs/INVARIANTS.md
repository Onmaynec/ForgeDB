# Инварианты ForgeDB

Статусы:

- **обеспечен** — код и автоматические тесты прошли обязательный release gate версии;
- **draft** — код и тесты добавлены в feature/release-ветку, но полный gate ещё не подтверждён;
- **частично** — обеспечен только фундамент инварианта;
- **запланирован** — feature нельзя заявлять до указанного milestone.

| ID | Инвариант | Статус | Доказательство |
|---|---|---|---|
| I-001 | Один каталог базы одновременно открыт не более чем одним процессом ForgeDB. | обеспечен | native lock tests; lock удерживается до `close`/drop/process exit |
| I-002 | Номер persisted-файла не переиспользуется после публикации в MANIFEST. | обеспечен для v0.2.0 | единый `next_file_number`; MANIFEST validation; reserved flush generation |
| I-003 | Sequence number не равен нулю; persisted-диапазоны возрастают и не пересекаются. | обеспечен | primitive tests, WAL replay validation, DisableWal gap test |
| I-004 | Подтверждённый WAL WriteBatch при recovery либо применяется полностью, либо отсутствует полностью. | обеспечен | один кадр на batch, full decode before apply, process crash tests |
| I-005 | Новое состояние становится видимым только после полного применения batch. | обеспечен | staged memtable apply под writer mutex |
| I-006 | MANIFEST никогда не ссылается на не полностью установленный SSTable. | обеспечен для v0.2.0 | SST build/sync/rename до installation edit; corruption и reopen tests |
| I-007 | SSTable неизменяем после установки. | обеспечен для v0.2.0 | immutable file lifecycle; checksum validation при open |
| I-008 | L1+ не содержит перекрывающихся диапазонов файлов. | запланирован v0.3.0 | VersionSet validation |
| I-009 | L0 point lookup проверяет файлы от нового к старому. | обеспечен для v0.2.0 | `l0_tables.iter().rev()`; overwrite-after-flush tests |
| I-010 | Snapshot не наблюдает sequence выше собственного. | запланирован v0.3.0 | snapshot model tests |
| I-011 | Compaction не удаляет версию, видимую активному snapshot. | запланирован v0.4.0 | compaction model tests |
| I-012 | Tombstone удаляется только после доказательства безопасности. | запланирован v0.4.0 | bottommost/snapshot tests |
| I-013 | Obsolete-файл сохраняется, пока используется Version, reader, compaction или checkpoint. | запланирован v0.4.0 | lifetime tests |
| I-014 | Каждая persisted-длина и offset проверяются до allocation или I/O. | обеспечен для V1 форматов v0.2.0 | malformed/limit/checksum tests |
| I-015 | Неизвестная будущая версия persisted-формата не открывается как текущая. | обеспечен для V1 форматов v0.2.0 | future-version rejection tests |
| I-016 | Verify никогда не изменяет базу. | запланирован v0.3.0 | verify inventory tests |
| I-017 | Salvage никогда не изменяет исходную базу. | запланирован v0.9.0 | salvage integration tests |
| I-018 | Normal recovery не продолжается после mid-file corruption. | обеспечен для WAL/SSTable/MANIFEST V1 | checksum/corruption tests, отсутствие resync |
| I-019 | DatabaseId каждого WAL и SSTable совпадает с `IDENTITY`. | обеспечен для v0.2.0 | header validation и mismatch tests |
| I-020 | Ошибка storage write видима последующим записям. | обеспечен для WAL/MANIFEST/flush v0.2.0 | fatal writer state после append/sync error |
| I-021 | Только последний физически неполный WAL-кадр может быть отброшен. | обеспечен | trailing partial tests |
| I-022 | Projected memtable memory проверяется до WAL mutation. | обеспечен | preflight accounting tests |
| I-023 | `Sync` выполняет WAL sync до memtable visibility. | обеспечен | write order и process-abort test |
| I-024 | `close()` освобождает WAL writer и системную блокировку только после успешного sync. | обеспечен для v0.2.0 | close/reopen tests; closed flag публикуется после sync |
| I-025 | После WAL rotation previous и current поколения восстанавливаются в разные memtable. | обеспечен для v0.2.0 | rotated-recovery test; mutable → immutable → L0 read order |
| I-026 | MANIFEST-owned current/previous WAL не пересоздаётся и не игнорируется при отсутствии. | обеспечен для v0.2.0 | missing-active-WAL unit и integration regression tests |
| I-027 | Старый WAL удаляется только после успешного MANIFEST installation edit и sync. | обеспечен для v0.2.0 | flush protocol; reopen-without-obsolete-WAL test |
| I-028 | Повторный flush использует то же reserved SST generation и может удалить только доказанный orphan этого номера. | обеспечен для v0.2.0 | generation validation; deterministic orphan cleanup |
| I-029 | Параллельные flush не строят два SSTable для одной immutable memtable. | обеспечен для v0.2.0 | `flush_in_progress`; `Busy` для второго flush и close |
| I-030 | Перед WAL rotation текущее WAL-поколение синхронизируется. | обеспечен для v0.2.0 | explicit WAL sync до создания rotation edit |
| I-031 | Initial WAL создан и синхронизирован до публикации `CURRENT`. | обеспечен для v0.2.0 | MANIFEST bootstrap wrapper; WAL-before-CURRENT unit/integration tests |
| I-032 | Header-only orphan WAL можно повторно использовать только после проверки UUID, file number, отсутствия complete frame и partial tail. | обеспечен для v0.2.0 | strict idempotent `create_wal`; empty/non-empty orphan tests |
| I-033 | При отсутствии `CURRENT` bootstrap cleanup удаляет только reserved `MANIFEST-000001`, его temp и `CURRENT.tmp`; неизвестные пользовательские файлы сохраняются. | обеспечен для v0.2.0 | bootstrap orphan cleanup test, включая `user.data` |
| I-034 | Существующий `CURRENT` запрещает повторную initial MANIFEST initialization. | обеспечен для v0.2.0 | existing-current rejection test |
| I-035 | Release gate использует зафиксированный dependency graph. | обеспечен для v0.2.0 | синхронизированный `Cargo.lock`; все compile/test/build команды CI запускаются с `--locked` |

## Правило изменения

Новый persisted-протокол или изменение write/recovery path обязаны обновить эту таблицу и добавить автоматическое
доказательство соответствующего инварианта. Одной документации недостаточно для статуса **обеспечен**.
