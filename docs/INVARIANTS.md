# Инварианты ForgeDB

Статусы:

- **обеспечен** — существует код и зелёное автоматическое доказательство выпущенной версии;
- **реализован в draft** — код и tests присутствуют в незавершённой ветке, но release gate ещё не подтверждён;
- **частично** — обеспечен только фундамент инварианта;
- **запланирован** — feature нельзя заявлять до указанного milestone.

| ID | Инвариант | Статус | Доказательство |
|---|---|---|---|
| I-001 | Один каталог базы одновременно открыт не более чем одним процессом ForgeDB. | обеспечен | native lock tests; lock удерживается до `close`/drop/process exit |
| I-002 | Номер persisted-файла никогда не переиспользуется. | реализован в draft v0.2.0 | MANIFEST next_file_number, numbered WAL/SST, flush tests; CI gate не подтверждён |
| I-003 | Sequence number не равен нулю; persisted-диапазоны возрастают и не пересекаются. | обеспечен | primitive tests, WAL replay validation, DisableWal gap test |
| I-004 | Подтверждённый WAL WriteBatch при recovery либо применяется полностью, либо отсутствует полностью. | обеспечен | один кадр на batch, full decode before apply, process crash tests |
| I-005 | Новое состояние становится видимым только после полного применения batch. | обеспечен | staged memtable apply под writer mutex |
| I-006 | MANIFEST никогда не ссылается на не полностью установленный SSTable. | реализован в draft v0.2.0/v0.3.0 | two-phase rotation/install, preflight VersionSet и crash-recovery tests; CI gate не подтверждён |
| I-007 | SSTable неизменяем после установки. | реализован в draft v0.2.0 | temp build, sync, atomic rename и metadata validation; CI gate не подтверждён |
| I-008 | L1+ не содержит перекрывающихся диапазонов файлов. | реализован в draft v0.3.0 | VersionSet sorted/non-overlap validation и overlap tests; CI gate не подтверждён |
| I-009 | L0 point lookup проверяет файлы от нового к старому. | реализован в draft v0.3.0 | VersionSet newest-first и read-path tests; CI gate не подтверждён |
| I-010 | Snapshot не наблюдает sequence выше собственного. | реализован в draft v0.3.0 | MVCC memtable, snapshot-aware SST scan и model tests; CI gate не подтверждён |
| I-011 | Compaction не удаляет версию, видимую активному snapshot. | запланирован v0.4.0 | compaction model tests |
| I-012 | Tombstone удаляется только после доказательства безопасности. | запланирован v0.4.0 | bottommost/snapshot tests |
| I-013 | Obsolete-файл сохраняется, пока используется Version, reader, compaction или checkpoint. | запланирован v0.4.0 | lifetime tests |
| I-014 | Каждая persisted-длина и offset проверяются до allocation или I/O. | реализован для SST/MANIFEST draft; обеспечен для Internal Key V1, IDENTITY V1 и WAL V1 | bounded decoders и malformed/limit tests |
| I-015 | Неизвестная будущая версия persisted-формата не открывается как текущая. | обеспечен для released formats; реализован в draft для SST/MANIFEST | future-version rejection tests |
| I-016 | Verify никогда не изменяет базу. | реализован в draft v0.3.0 | inventory/deep-verify reports и read-only tests; CI gate не подтверждён |
| I-017 | Salvage никогда не изменяет исходную базу. | запланирован v0.9.0 | salvage integration tests |
| I-018 | Normal recovery не продолжается после mid-file corruption. | обеспечен для WAL V1; реализован в draft для SST/MANIFEST | checksum/corruption tests, отсутствие resync |
| I-019 | DatabaseId каждого live persisted-файла совпадает с `IDENTITY`. | реализован в draft v0.2.0/v0.3.0 | WAL/SST/MANIFEST header validation; CI gate не подтверждён |
| I-020 | Ошибка storage write видима последующим записям. | реализован в draft для WAL, MANIFEST и flush | fatal write state и retry-safe flush state machine; CI gate не подтверждён |
| I-021 | Только последний физически неполный WAL-кадр может быть отброшен. | обеспечен | trailing partial tests |
| I-022 | Projected memtable memory проверяется до WAL mutation. | обеспечен | preflight accounting tests |
| I-023 | `Sync` выполняет WAL sync до memtable visibility. | обеспечен | write order и process-abort test |
| I-024 | `close()` освобождает WAL writer и системную блокировку. | обеспечен | reopen tests после close |
| I-025 | Range scan использует один snapshot sequence для всех sources. | реализован в draft v0.3.0 | immutable VersionSet, ReadOptions и scan model tests; CI gate не подтверждён |
| I-026 | SSTable range source не материализует полный файл. | реализован в draft v0.3.0 | `SstEntryCursor`: decoded index + один resident data block; multi-block и multi-file tests; CI gate не подтверждён |
| I-027 | Recovery plan и inventory не изменяют каталог базы. | реализован в draft v0.3.0 | process lock, read-only inspection и before/after directory tests; CI gate не подтверждён |

## Правило изменения

Новый persisted-протокол или изменение write/recovery/read path обязаны обновить эту таблицу и добавить автоматическое
доказательство соответствующего инварианта. Одной документации недостаточно для статуса **обеспечен**.
