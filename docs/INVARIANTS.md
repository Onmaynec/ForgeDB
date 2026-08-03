# Инварианты ForgeDB

Статусы:

- **обеспечен** — существует код и тесты текущей версии;
- **частично** — обеспечен только фундамент инварианта;
- **запланирован** — feature нельзя заявлять до указанного milestone.

| ID | Инвариант | Статус | Доказательство |
|---|---|---|---|
| I-001 | Один каталог базы одновременно открыт не более чем одним процессом ForgeDB. | обеспечен | native lock tests; lock удерживается до `close`/drop/process exit |
| I-002 | Номер persisted-файла никогда не переиспользуется. | запланирован v0.2.0 | MANIFEST и VersionSet |
| I-003 | Sequence number не равен нулю; persisted-диапазоны возрастают и не пересекаются. | обеспечен | primitive tests, WAL replay validation, DisableWal gap test |
| I-004 | Подтверждённый WAL WriteBatch при recovery либо применяется полностью, либо отсутствует полностью. | обеспечен | один кадр на batch, full decode before apply, process crash tests |
| I-005 | Новое состояние становится видимым только после полного применения batch. | обеспечен | staged memtable apply под writer mutex |
| I-006 | MANIFEST никогда не ссылается на не полностью установленный SSTable. | запланирован v0.2.0 | installation crash matrix |
| I-007 | SSTable неизменяем после установки. | запланирован v0.2.0 | SST lifecycle tests |
| I-008 | L1+ не содержит перекрывающихся диапазонов файлов. | запланирован v0.3.0 | VersionSet validation |
| I-009 | L0 point lookup проверяет файлы от нового к старому. | запланирован v0.3.0 | read-path tests |
| I-010 | Snapshot не наблюдает sequence выше собственного. | запланирован v0.3.0 | snapshot model tests |
| I-011 | Compaction не удаляет версию, видимую активному snapshot. | запланирован v0.4.0 | compaction model tests |
| I-012 | Tombstone удаляется только после доказательства безопасности. | запланирован v0.4.0 | bottommost/snapshot tests |
| I-013 | Obsolete-файл сохраняется, пока используется Version, reader, compaction или checkpoint. | запланирован v0.4.0 | lifetime tests |
| I-014 | Каждая persisted-длина и offset проверяются до allocation или I/O. | обеспечен для Internal Key V1, IDENTITY V1 и WAL V1 | malformed/limit tests |
| I-015 | Неизвестная будущая версия persisted-формата не открывается как текущая. | обеспечен | future-version rejection tests |
| I-016 | Verify никогда не изменяет базу. | запланирован v0.3.0 | verify inventory tests |
| I-017 | Salvage никогда не изменяет исходную базу. | запланирован v0.9.0 | salvage integration tests |
| I-018 | Normal recovery не продолжается после mid-file corruption. | обеспечен для WAL V1 | checksum/corruption tests, отсутствие resync |
| I-019 | DatabaseId WAL совпадает с `IDENTITY`. | обеспечен для WAL V1 | header validation |
| I-020 | Ошибка storage write видима последующим записям. | обеспечен для WAL writer; flush запланирован v0.2.0 | fatal write state |
| I-021 | Только последний физически неполный WAL-кадр может быть отброшен. | обеспечен | trailing partial tests |
| I-022 | Projected memtable memory проверяется до WAL mutation. | обеспечен | preflight accounting tests |
| I-023 | `Sync` выполняет WAL sync до memtable visibility. | обеспечен | write order и process-abort test |
| I-024 | `close()` освобождает WAL writer и системную блокировку. | обеспечен | reopen tests после close |

## Правило изменения

Новый persisted-протокол или изменение write/recovery path обязаны обновить эту таблицу и добавить автоматическое
доказательство соответствующего инварианта. Одной документации недостаточно для статуса **обеспечен**.
