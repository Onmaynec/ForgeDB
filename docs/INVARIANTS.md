# Инварианты ForgeDB

Статусы: **обеспечен** — зелёное доказательство released version; **реализован в draft** — код/tests есть, release gate не подтверждён; **запланирован** — будущий milestone.

| ID | Инвариант | Статус |
|---|---|---|
| I-001 | Один process lock на каталог. | обеспечен |
| I-002 | File numbers монотонны и не переиспользуются. | реализован в draft v0.2 |
| I-003 | Sequence ненулевые и не откатываются. | обеспечен |
| I-004 | WAL batch replay атомарен. | обеспечен |
| I-005 | Batch видим только после полного apply. | обеспечен |
| I-006 | MANIFEST не ссылается на неустановленный SST. | реализован в draft v0.2/v0.3 |
| I-007 | Installed SST immutable. | реализован в draft v0.2 |
| I-008 | L1+ ranges не перекрываются. | реализован в draft v0.3 |
| I-009 | L0 читается newest-first. | реализован в draft v0.3 |
| I-010 | Snapshot не видит более новые sequence. | реализован в draft v0.3 |
| I-011 | Compaction сохраняет active snapshot versions. | запланирован v0.4 |
| I-012 | Tombstone удаляется только после доказательства. | запланирован v0.4 |
| I-013 | Obsolete file живёт, пока удерживается reader/version. | запланирован v0.4 |
| I-014 | Persisted lengths/offsets bounded до allocation/I/O. | реализован в draft для SST/MANIFEST; обеспечен для released formats |
| I-015 | Future format versions отклоняются. | реализован/обеспечен по текущим форматам |
| I-016 | Verify не изменяет базу. | реализован в draft v0.3 |
| I-017 | Salvage не изменяет source database. | запланирован v0.9 |
| I-018 | Mid-file corruption блокирует normal recovery. | реализован/обеспечен по текущим форматам |
| I-019 | DatabaseId live files совпадает с IDENTITY. | реализован в draft v0.2/v0.3 |
| I-020 | Storage write failure становится fatal для writer. | реализован в draft |
| I-021 | Отбрасывается только final partial WAL frame. | обеспечен |
| I-022 | Memtable limit проверяется до WAL mutation. | обеспечен |
| I-023 | Sync WAL предшествует memtable visibility. | обеспечен |
| I-024 | close освобождает writers и lock. | обеспечен |
| I-025 | Range scan использует один snapshot. | реализован в draft v0.3 |
| I-026 | SST range source не materialize-ит полный файл. | реализован в draft v0.3 |
| I-027 | Inventory/recovery plan не изменяют каталог. | реализован в draft v0.3 |
| I-028 | Streaming wrapper не меняет API/persisted state. | реализован в draft; consolidation остаётся release gate |

Изменение write/recovery/read path обязано добавлять test и обновлять эту таблицу. Одной документации недостаточно для статуса **обеспечен**.
