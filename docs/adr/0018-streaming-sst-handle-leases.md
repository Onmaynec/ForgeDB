# ADR 0018 — Общий SST handle pool для потоковых читателей

- Статус: принято для v0.8.2
- Дата: 2026-11-14

## Контекст

В v0.8.1 native LRU `TableHandlePool` ограничивал удерживаемые `File` для point GET. Streaming `SstEntryCursor` после построения индекса хранил собственный `std::fs::File` на весь срок жизни итератора, даже когда потреблялись данные из уже загруженного блока. Compaction загружала входные SST через отдельное `table_scan::read_entries` с `fs::read`. В сценарии со многими L0 и старыми snapshot iterators число дескрипторов оставалось пропорционально числу таких курсоров.

## Решение

`SstEntryCursor::open_pooled` использует существующий безопасный конструктор для начальной проверки физического SST (header, footer, Bloom/filter, index, properties, CRC, bounds и первый data block), а затем закрывает **собственный** `File` и сохраняет `Arc<TableHandlePool>` с проверенной `DatabaseId` и `TableMeta`. Это не меняет codec или сохранённый индекс.

При `advance`, когда заканчивается текущий data block, курсор вызывает `TableHandlePool::with_file` для чтения следующего блока. Под одним mutex pool либо повторно использует resident File, либо сначала выселяет LRU File, затем открывает SST заново с проверкой file size, database identity, file number, sequence range и CRC заголовка. Прежняя проверка trailer/checksum и строгий decoder data block сохраняются. Ошибка чтения возвращается вызывающему коду и не превращается в пустой итератор.

`StreamingDatabase::scan` использует pool текущего `VersionSet`. Старый iterator по-прежнему удерживает `Arc<VersionSet>`, а значит и `Arc<TableReader>` физического SST; сам факт eviction из pool не даёт права удалять файл. `cleanup_obsolete_sstables` проверяет прежний `Weak<TableReader>`, после чего закрывает pooled FD **перед** Windows `remove_file`.

Compaction inputs переходят на `table_scan::read_entries_pooled` поверх того же cursor. Это убирает full-file read и выделенный input File, но итоговые entries всё ещё материализуются для существующего merge, ограниченного `max_compaction_bytes`. Новый алгоритм streaming output или полный external merge не заявляется.

## Лимит и владение дескрипторами

`Options::table_cache_capacity` задаёт жёсткий максимум одновременно **resident** File внутри одного `TableHandlePool`, общий для point GET, последующих data blocks streaming scan и compaction inputs одного Database. Pool синхронизирует доступ mutex; он может ограничить throughput при конкуренции за I/O, что нужно измерять отдельно.

Граница не является process-wide OS FD cap. При инициализации отдельного cursor или `TableReader` по-прежнему кратковременно открывается File для полной проверки metadata и первого блока; этот File не хранится после `open_pooled`, но **в момент инициализации** может сосуществовать с cached pool handles. Также отдельно открываются SST output writer, WAL/MANIFEST/LOCK, глубокая verification и sibling Column Families, каждая со своим пулом. Эти дескрипторы не отражаются в `table_handle_stats`. Метрика `open_handles` — резидентность пула, а не число файлов всего процесса.

Простой `evict_table` удаляет file number из cache и из `seen` учёта reopen, что предотвращает неограниченное накопление идентификаторов уже окончательно retired SST, но не удаляет файл само по себе.

## Совместимость и безопасность

- Никаких новых persisted WAL/SSTable/MANIFEST/CF_REGISTRY, нет миграции старых баз.
- Проверка `SSTable` identity при каждом pooled reopen и CRC32C блока остаётся. Неподтверждённая файловая подмена вне защищённой файловой системы не считается поддержанным сценарием.
- Старые snapshots/iterators сохраняют owner lease. Нельзя удалять orphan SST по догадке после restart.
- `table_cache_capacity` не является global FD cap для нескольких баз, metadata verification и write paths: общая Issue #31 остаётся открытой до реализации единой квоты.

## Validation

Тесты: 48 SST × 10 key entries с capacity=1 и проверкой сортировки/reopen; четыре параллельных GET-worker вместе со streaming scan; компактация многоблочных входов и old snapshot iterator + Windows-safe cleanup. Дополнительно `#[ignore]` ручной stress на 1024 SST, не входящий в обязательный CI. Проверки на Ubuntu/Windows stable + Rust 1.89, rustfmt, strict Clippy, tests и release build.

См. `docs/releases/V0.8.2.md`, `docs/OPTIONS.md`, `docs/INVARIANTS.md`.
