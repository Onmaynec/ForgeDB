# Состояние параметров Options в ForgeDB v0.8.1

Значение по умолчанию — не доказательство того, что параметр управляет указанным ресурсом. При `ColumnFamilies` каждый child DB получает собственный экземпляр ресурсов и cache (включая `block_cache_size`): это не общий лимит RAM/FD для группы families. Ниже приведена текущая граница реализации.

| Параметр | Состояние | Фактическое действие |
|---|---|---|
| `memtable_size` | используется | Ограничивает projected mutable memtable; публичные записи при его достижении делают один synchronous flush/retry |
| `block_cache_size` | используется | Лимит decoded-block LRU point-read cache, `0` отключает хранение |
| `table_cache_capacity` | используется частично | LRU лимит одновременно resident native SST handles в point-read pool одного Database (default 512). Не ограничивает cursors, compaction, WAL/MANIFEST/LOCK и sibling Column Families |
| `max_background_flushes` | зарезервирован | Фоновые flush workers отсутствуют |
| `max_background_compactions` | зарезервирован | Фоновые compaction workers отсутствуют |
| `wal_file_size` | зарезервирован | Не является активным триггером automatic WAL rotation по размеру |
| `max_compaction_bytes` | используется | Ограничивает входные байты одного synchronous compaction job |
| `l0_compaction_trigger` / slowdown / stop | используется | Определяет поведение synchronous leveled compaction и write stall |
| `verify_checksums` | используется | Проверка checksum при обычных read paths |
| `paranoid_checks` | частично зарезервирован | Не заменяет обязательные проверки форматов и инвариантов; не стоит считать его новым strict recovery mode |

После v0.8.1 `Database::table_handle_stats()` и `ColumnFamilies::table_handle_stats(id)` возвращают capacity/open_handles, hits/misses, evictions/reopens. Они не измеряют process-wide FD consumption. Сам point-read pool удерживает не более указанной capacity; открытые streaming cursor/compaction файлы в этот счётчик не включены.

Все параметры сохраняются в публичном `Options` для совместимости API. Не следует заявлять worker concurrency, table-cache residency или bounded total RSS только по наличию соответствующего поля.

Подробнее о cache: [CACHE_AND_PERFORMANCE.md](CACHE_AND_PERFORMANCE.md). Об условиях записи: [WRITE_PATH.md](WRITE_PATH.md). Политика проверок: [VALIDATION.md](VALIDATION.md).
