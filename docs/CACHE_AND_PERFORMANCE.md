# Cache and Performance — ForgeDB v0.6.0

## Назначение

v0.6.0 ускоряет point reads и optimistic transaction conflict validation, не меняя WAL, SSTable и MANIFEST persisted layouts. Основной принцип: использовать проверенный индекс SSTable и Bloom filter вместо повторной материализации всех entries при каждом `get`.

## Путь чтения

Публичный `Database::get` ищет значение в mutable memtable, затем в pending immutable memtable и после этого в актуальном `VersionSet`. Проверяются L0-файлы от нового к старому и non-overlapping L1+.

Для каждого подходящего SSTable вызывается `TableReader::get_at_cached`:

1. Проверяются границы ключей из MANIFEST и Bloom filter, чтобы исключить заведомо неподходящие файлы.
2. По индексным separator keys находится первый data block, в котором может начаться искомый user key.
3. Block декодируется и проверяется, затем результаты могут попасть в LRU-кэш.
4. Если версии одного user key занимают несколько data blocks, чтение продолжается до первой версии, видимой на snapshot sequence, либо до ключа, следующего за искомым.
5. Tombstone скрывает предыдущие значения, в том числе при snapshot reads.

Внутри L1+ выбранный файл находится бинарным поиском по непересекающимся диапазонам. Отсутствующий ключ не требует сканирования всей таблицы.

Transaction validation использует `latest_sequence_after_cached`, сравнивает committed sequence с BEGIN snapshot и сохраняет write-write conflict guarantees v0.5.0, включая ABA. Прежний `read_entries` остаётся для compaction и deep verification, но не используется на обычном point lookup.

## Bounded decoded-block cache

`Options::block_cache_size` задаёт верхний бюджет **учтённых декодированных SST data blocks**, в байтах. Значение `0` отключает хранение блоков в LRU. Кэш создаётся для каждого открытого `Database` и разделяется поколениями его `VersionSet`, включая snapshots и транзакции.

Ключ кэша — (`file_number`, `block_index`); номера файлов не переиспользуются. В кэш допускаются только успешно проверенные и декодированные данные. Данные с ошибкой checksum/decoder не помещаются в кэш. При заполнении бюджета вытесняется давно не использовавшийся block. Слишком большой block можно прочитать, но нельзя удерживать в кэше поверх бюджета.

Для мониторинга:

```rust
let database = forgedb::Database::open("./data", forgedb::Options::default())?;
let stats = database.cache_stats()?;
println!("hits={}, misses={}, evictions={}", stats.hits, stats.misses, stats.evictions);
```

`CacheStats` также содержит `bypasses`, `resident_blocks`, `resident_bytes`, `capacity_bytes`. Счётчики относятся только к данному открытому экземпляру. Они не являются метриками disk reads, throughput или общего RSS.

**Текущая граница учёта памяти:** `TableReader::open` по-прежнему заранее загружает физические SSTable bytes и метаданные reader в память. `block_cache_size` НЕ ограничивает этот исходный буфер и не является лимитом всего использования RAM. Накладные расходы коллекций и временных concurrent reads тоже не входят в `resident_bytes`. `Options::table_cache_capacity` остаётся зарезервированной опцией; полноценный lazy file reader/table cache пока не реализован.

## Range scan

Публичный `scan` использует прежние block-at-a-time SST cursors и merge iterator с range bounds. Он не использует кэш декодированных point-lookup blocks. Iterator удерживает выбранный `Arc<VersionSet>`, чтобы файлы оставались доступны до завершения чтения.

## Безопасная очистка retired SSTables

После successful leveled compaction MANIFEST V2 определяет новую live topology; только тогда input SSTable ставятся в локальную очередь retirement. Для каждого input хранится weak reference на соответствующий `TableReader`.

`Database::cleanup_obsolete_sstables()`:

- проверяет writable/open состояние базы и отсутствие concurrent flush/compaction;
- не удаляет SST, если его file number ещё присутствует в current VersionSet;
- откладывает удаление, пока остаётся хотя бы один reader указанного SSTable, включая reader из старой версии/итератора;
- удаляет только canonical SST names из списка, выведенного доказанным synced compaction edit текущего процесса;
- освобождает cached blocks удалённого файла, синхронизирует каталог после успешных удалений;
- возвращает `CleanupReport` с `removed`, `deferred`, `absent`, `failed`; неудачное физическое удаление можно повторить, не меняя логическую топологию.

Команда `forgedb compact` вызывает очистку после compaction и выводит результат. Cleanup не запускается автоматически на каждой записи или при открытии базы.

**Важная граница:** очередь retirement находится в памяти. После нового открытия ForgeDB не делает выводы из произвольного orphan SST и не удаляет его, даже если файл выглядит ненужным. Backups, unknown и user files не затрагиваются. Этот процесс-lifetime reaper не является полной cross-restart garbage collection и не должен выдаваться за неё.

## Воспроизводимые измерения

Benchmark без сторонних зависимостей:

```bash
cargo run --locked --release -p forgedb --example point_lookup_bench -- 8192 50000
```

Аргументы: число ключей и число point reads за проход. Временная база создаётся с согласованным набором ключей. Программа выполняет подготовку, cold и warm проходы, проверяет наличие каждого прочитанного ключа и выводит длительности и cache counters.

Сравнение проводят на одинаковом железе, Rust toolchain, объёме данных, cache budget и наборе запросов. Числа из одного запуска не являются гарантированным ускорением, а кэш ОС может влиять на cold/warm сравнение. Необходимые профили: cache disabled, small cache с eviction, warm-cache, read-after-flush, transaction conflict after SST flush. CI выполняет короткий smoke-run без числового порога производительности.

## Протоколы и диагностика

Persisted-format impact: **нет**. WAL V1, SSTable V1, MANIFEST V2 (reader также V1), CURRENT V1. Data blocks не переписываются при cache fill или cleanup. `verify(VerifyOptions::deep())` проверяет содержимое исходных live SST независимым полным reader path; кэш не скрывает persisted corruption.

См. [READ_PATH.md](READ_PATH.md), [COMPACTION.md](COMPACTION.md), [RECOVERY.md](RECOVERY.md), [INVARIANTS.md](INVARIANTS.md) и ADR 0012/0013.
