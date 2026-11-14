# ADR 0019 — Performance Engine: индекс LRU и снижение копирований MVCC

- Статус: принято для v0.9.0
- Дата: 2026-11-14

## Исходные проблемы

До v0.9.0 в native `TableHandlePool` при переполнении `table_cache_capacity` выполнялся линейный поиск `opened.iter().min_by_key` по всем открытым SST. При увеличении вместимости пул оставался корректным, но стоимость каждого cache miss/eviction росла как O(N). Остальные части хранилища правильно использовали SSD/LSM index, однако контроль конфликтов транзакций `VersionSet::latest_sequence_after` просматривал каждую SST L1+ даже при строго непересекающихся key ranges. Merge `DbIterator::advance_source` копировал целый `TableEntry` с value для проверки монотонности одного `InternalKey`; `consume_item` копировал значения даже у MVCC версий, которые никогда не будут возвращены.

## Решения

### 1. Native SST read handle LRU с индексом O(log N)

`PoolState` продолжает хранить `BTreeMap<file_number, CachedHandle>`, но дополнительно хранит `BTreeSet<(last_access, file_number)>`, где `last_access` — счётчик, обновляемый на каждое обращение. При hit удаляется старый ключ индекса и вставляется новый. При miss, если pool заполнен, минимальный ключ берётся из `lru.iter().next()`: больше не нужен O(N) полный просмотр `opened`. При `evict_table` удаляются оба элемента, а также retired file number из `seen`.

**Сложность:** выбор LRU раньше O(N), после v0.9.0 O(log N) при индексированных BTreeMap/BTreeSet операциях; обращение к resident handle также O(log N) из-за поддержки индекса. Дополнительная память — одна индексная запись для каждого resident File, O(N). Изменения под тем же mutex сохраняют строгий `table_cache_capacity`, включая общие GET/streaming scan/compaction inputs и Windows-safe close-before-remove.

`clock` использует `saturating_add`: в недостижимом практически сценарии переполнения счётчика порядок разных записей с равным временем определяется `file_number`. Это ограничение документировано, гарантии реального времени LRU после более чем 2^64 обращений не заявляются.

### 2. MVCC merge без ненужных копирований value

`DbIterator::advance_source` копирует только `InternalKey` текущего элемента, чтобы сравнить его с предыдущим и проверить строго возрастающий порядок. Прежде копировалась вся `TableEntry`, включая потенциально большой value, перед каждым `cursor.advance()`.

`DbIterator::consume_item` сравнивает текущий ключ с heap без клонирования значения. Флаг `retain_value` задаётся по snapshot sequence и наличию уже выбранной видимой версии user key. Если запись заведомо не будет возвращена, её value не копируется. Для первой видимой версии остаётся `TableEntry::clone`, необходимая до перемещения исходного cursor. Tombstone и ошибка damaged/unsorted source обрабатываются прежним путём. Мы **не** отменяем обязательную проверку порядка и не меняем приоритет или сопоставление snapshot.

### 3. Binary range lookup при optimistic conflict detection

`VersionSet::latest_sequence_after` сохраняет полный перебор релевантных L0 SST (ranges могут перекрываться), но проходит их newest-first, отсекая таблицы с `largest_sequence <= max(after, newest_found)`. На уровнях L1+ применяется `partition_point` по `largest_user_key` для выбора **единственного** кандидата с соответствующим диапазоном, как уже делал `get_at`. Это заменяет линейный проход всех файлов уровня на O(log N) поиск; фактическое SST I/O выполняется только при проверке выбранного кандидата.

Безопасность: непересекающиеся L1+ ranges проверяются `VersionSet::validate`. У L0 нельзя делать такое сокращение; MVCC sequencing и tombstone/ABA guarantees сохраняются, результат `latest_sequence_after` по-прежнему возвращает максимальную подтверждённую sequence после snapshot.

### 4. Без временного cloned SST handle vector при SCAN

Раньше `StreamingDatabase::scan` сначала вызывал `VersionSet::files_in_read_order()`, выделял `Vec<TableHandle>` вместимостью во все live SST и клонировал `Arc<TableReader>` каждого файла, хотя большинство SST могли не перекрываться запрошенным диапазоном. Теперь метод `VersionSet::files_for_scan()` возвращает ленивый заимствованный iterator: L0 newest-first, затем L1+ в порядке сортировки. `scan` проверяет overlap и открывает cursor только для подходящих SST, не материализуя весь список и не клонируя Arc для остальных файлов.

Это **устранение одного O(N) выделения массива и N клонирований Arc за создание scan**, но проверка диапазонов SST остаётся O(N): бинарного index seek для scan-start эта версия не обещает. `DbIterator` удерживает `Arc<VersionSet>`, поэтому borrowed handles используются только пока локальный `VersionSet` ещё жив, а возвращаемый iterator пиннит immutable поколение. Порядок чтения, oldest-snapshot semantics и source priority не меняются.

## Проверка и измерения

Автоматические регрессии:
- capacity=2, чередование трёх SST, контроль LRU/MRU, точных `hits/misses/evictions/reopens`;
- разноуровневые SST после compaction, контроль конфликтов snapshot transaction и reopen;
- большие values (128 KiB), многократные записи одного ключа, чтение старого snapshot и latest, tombstones.

`examples/performance_bench.rs` выполняет повторяемые загрузку, cold/warm GET и streaming scan, выводит p50/p95/p99, ops/sec и счётчики SST FD. CI smoke: `cargo run --locked --release -p forgedb --example performance_bench -- 256 1024`. Для предметной оценки рекомендуется запускать несколько повторов на одной машине, на одних данных и одном Rust toolchain, исключая variance ОС cache/background tasks. **Снижение Big-O не доказывает улучшение wall-clock latency**: дополнительный индекс и mutex могут стоить дороже при маленьком N.

## Граница версии

Никакой новый persisted WAL/SSTable/MANIFEST/CF_REGISTRY, формат внутреннего ключа или миграция. Общий pool не становится process-wide FD budget: initial metadata opens, WAL/MANIFEST/LOCK, SST writes, deep verify и отдельные named CF остаются вне квоты (#31). Mixed-family atomic transactions не входят (#35). Новый benchmark не является performance SLA и не подменяет paired A/B исследование.

См. `docs/releases/V0.9.0.md`, `docs/PERFORMANCE_BENCHMARK.md`, `docs/INVARIANTS.md`.
