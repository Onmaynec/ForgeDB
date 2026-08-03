# Инварианты ForgeDB

Статусы:

- **обеспечен** — код и тесты прошли release gate выпущенной версии;
- **draft** — код и тесты добавлены, но полный release gate ещё не подтверждён;
- **частично** — обеспечен только фундамент инварианта;
- **запланирован** — feature нельзя заявлять до указанного milestone.

| ID | Инвариант | Статус | Доказательство |
|---|---|---|---|
| I-001 | Один каталог базы одновременно открыт не более чем одним процессом ForgeDB. | обеспечен | native lock tests; lock удерживается до `close`/drop/process exit |
| I-002 | Номер persisted-файла никогда не переиспользуется. | draft v0.2.0 | MANIFEST `next_file_number`, WAL/SST generations, VersionSet duplicate checks |
| I-003 | Sequence number не равен нулю; persisted-диапазоны возрастают и не пересекаются. | обеспечен | primitive tests, WAL replay validation, DisableWal gap test |
| I-004 | Подтверждённый WAL WriteBatch при recovery либо применяется полностью, либо отсутствует полностью. | обеспечен | один кадр на batch, full decode before apply, process crash tests |
| I-005 | Новое состояние становится видимым только после полного применения batch. | обеспечен | staged memtable apply под writer mutex |
| I-006 | MANIFEST никогда не ссылается на не полностью установленный SSTable. | draft v0.2.0 | temp write, file sync, atomic rename, directory sync, installation edit |
| I-007 | SSTable неизменяем после установки. | draft v0.2.0 | immutable file lifecycle и reader validation |
| I-008 | L1+ не содержит перекрывающихся диапазонов файлов. | draft v0.3.0 | `validate_level_metadata`, overlap tests |
| I-009 | L0 point lookup проверяет файлы от нового к старому. | draft v0.3.0 | immutable VersionSet и newest-first traversal |
| I-010 | Snapshot не наблюдает sequence выше собственного. | draft v0.3.0 | MVCC memtable, SST snapshot scanner, snapshot/merge tests |
| I-011 | Compaction не удаляет версию, видимую активному snapshot. | запланирован v0.4.0 | compaction model tests |
| I-012 | Tombstone удаляется только после доказательства безопасности. | запланирован v0.4.0 | bottommost/snapshot tests |
| I-013 | Obsolete-файл сохраняется, пока используется Version, reader, compaction или checkpoint. | частично v0.3.0 | readers удерживают `Arc<VersionSet>`; полный obsolete lifetime требует compaction |
| I-014 | Каждая persisted-длина и offset проверяются до allocation или I/O. | draft для SSTable/MANIFEST; обеспечен для Internal Key/IDENTITY/WAL | bounded codecs, malformed/limit tests |
| I-015 | Неизвестная будущая версия persisted-формата не открывается как текущая. | draft для SSTable/MANIFEST; обеспечен для ранних форматов | future-version rejection tests |
| I-016 | Verify никогда не изменяет базу. | draft v0.3.0 | read-only inventory и deep scanner |
| I-017 | Salvage никогда не изменяет исходную базу. | запланирован v0.9.0 | salvage integration tests |
| I-018 | Normal recovery не продолжается после mid-file corruption. | draft для SSTable/MANIFEST; обеспечен для WAL V1 | checksum/corruption tests, отсутствие resync |
| I-019 | DatabaseId каждого persisted-файла совпадает с `IDENTITY`. | draft для SSTable/MANIFEST; обеспечен для WAL V1 | header validation |
| I-020 | Ошибка storage write видима последующим записям. | draft для MANIFEST/flush; обеспечен для WAL writer | fatal writer state |
| I-021 | Только последний физически неполный WAL или MANIFEST frame может быть отброшен. | draft для MANIFEST; обеспечен для WAL | trailing partial tests |
| I-022 | Projected memtable memory проверяется до WAL mutation. | обеспечен | preflight accounting tests |
| I-023 | `Sync` выполняет WAL sync до memtable visibility. | обеспечен | write order и process-abort test |
| I-024 | `close()` освобождает WAL writer и системную блокировку. | обеспечен | reopen tests после close |
| I-025 | Одна read-операция использует один зафиксированный snapshot sequence во всех sources. | draft v0.3.0 | `ReadOptions`, capture под state mutex, source-order tests |
| I-026 | Snapshot другой базы или другого open handle отклоняется. | draft v0.3.0 | persisted identity + per-handle identity, foreign snapshot test |
| I-027 | File inventory и verify не удаляют orphan, obsolete, temporary или unknown objects. | draft v0.3.0 | non-destructive inventory API и documentation |
| I-028 | Flush retry не публикует VersionSet раньше durable MANIFEST installation edit. | draft v0.3.0 | VersionSet preflight, MANIFEST append, atomic in-memory publish |
| I-029 | Ошибка flush всегда снимает transient `flush_in_progress`, сохраняя pending recovery state. | draft v0.3.0 | explicit flush state machine и retry path |

## Правило изменения

Новый persisted-протокол или изменение write/recovery path обязаны обновить эту таблицу и добавить автоматическое
доказательство соответствующего инварианта. Одной документации недостаточно для статуса **обеспечен**.

Статус **draft** не является заявлением о выпуске: он сохраняется до успешной Linux/Windows/MSRV матрицы и release job.
