# ADR 0017 — LRU-кэш дескрипторов для point-read SST

- Статус: принято для v0.8.1
- Дата: 2026-11-14

## Контекст

Начиная с v0.6.2 SSTable data blocks читаются лениво, однако каждый открытый `TableReader` удерживал физический `RandomAccessReader` до завершения жизни этого reader. `VersionSet` в L0/L1+ содержит `Arc<TableReader>`; десятки и тысячи SSTable означали столь же много открытых OS handles. Поле `Options::table_cache_capacity` было зарезервировано, фактически FD не ограничивало.

При этом reference-aware очистка после compaction требует не удалять SST, который удерживается старым `VersionSet` или iterator, даже если OS handle закрыт. Простая замена readers на новые без сохранения lifetime ownership нарушила бы эту гарантию. Особенно опасно удаление файла в Windows при ещё открытом read handle.

## Решение v0.8.1

В `forgedb-sstable` добавлен `TableHandlePool`, независимый от decoded-block `BlockCache`. Это один `Arc` на открытый Database, общий для всех поколений `VersionSet` и добавленных flush/compaction SST. Cache size задаётся `Options::table_cache_capacity` (по умолчанию 512, допускаются только положительные значения).

1. `TableReader::open` по-прежнему физически открывает файл и полностью валидирует header/footer, index/filter/properties checksums. Только после проверки reader переводится в native pooled режим, начальный FD закрывается. Новый `VersionSet` не оставляет по одному открытому FD на каждый SST.
2. При point lookup reader проверяет Bloom и индекс. Если нужный data block отсутствует в decoded-block cache, выполняется запрос в общий `TableHandlePool`. Первый запрос открывает native File, сверяет физический размер, DB identity, SST number, header checksum и sequence range с `TableMeta`. Затем data block читается по известному `BlockHandle` с прежней CRC32C-проверкой.
3. Повторное обращение к открытому File обновляет LRU timestamp. При заполненном budget на miss закрывается наименее недавно использовавшийся дескриптор, затем открывается новый. При повторном обращении к выселенному SST проверка выполняется заново; decode/checksum failures не попадают в block cache.
4. Один `Mutex` сериализует `seek/read`, открытие и eviction. Внутри pool одновременное число открытых файлов `<= table_cache_capacity` без переполнений под concurrency. Это безопаснее, но может ограничивать параллелизм точечных SST reads; performance gain без measurements не заявляется.
5. `Arc<TableReader>` продолжает определять lifetime данных и проверку `Weak<TableReader>` для retired SST, а не количество открытых OS handles. При очистке после успешного MANIFEST deletion и отсутствия live readers pool закрывает файловый handle **до** `remove_file`. Никакого автоматического удаления неизвестных SST после restart.
6. `Database::table_handle_stats()` выдаёт `TableHandleStats`: capacity, open_handles, hits, misses, evictions, reopens. Это метрики только native point-read handles данного DB.

## Ограничения и риски

- Scope отдельного пула **только point lookups** одного Database. Streaming range iterator `SstEntryCursor`, компактация, WAL/MANIFEST, temporary output и другие именованные `ColumnFamilies` не имеют общего процессного FD budget.
- `block_cache_size` — отдельный бюджет decoded blocks: попадание в decoded block cache может вообще не открыть FD.
- Header/identity/size повторно проверяются при reopen; CRC32C проверяется при последующем block read. Полной криптографической гарантии против злоумышленной подмены файла теми же identity/sequence и пересчитанными checksum нет. SST immutable files должны защищаться файловой системой.
- При чисто point-read нагрузке глобальный Mutex может вызывать contention; это осознанный стартовый контракт, оптимизации concurrency должны сохранять bounded handles и safety read leases.
- Не завершает Issue #31 в полном объёме, поскольку общий process-wide budget по всем потокам чтения и child databases не реализован.
- Не меняет persisted WAL V1, SSTable V1, MANIFEST writer V2/reader V1+V2, CURRENT V1, CF_REGISTRY V1. Миграция не требуется.

## Validation

Integration: budget 1, два SST и возврат к вытесненному файлу; reopen после close/recovery; compaction + pinned old scan + Windows-safe cleanup; MVCC snapshot/transaction conflicts через eviction. Unit: физическое повреждение SST header после eviction, reopen отвергает corruption и после восстановления снова возвращает данные. Платформы: Ubuntu/Windows stable + Rust 1.89, strict Clippy, tests/doctests/build.

См. `docs/releases/V0.8.1.md`, `docs/OPTIONS.md` и `docs/INVARIANTS.md`.
