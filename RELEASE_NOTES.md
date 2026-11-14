# ForgeDB v0.8.2 — Streaming SST Handle Pool

Статус: GitHub Pre-release.

В этой версии продолжаю работу над контролем количества открытых SST-файлов. В v0.8.1 общий кэш дескрипторов использовался точечными чтениями (`GET`), но потоковые курсоры `SCAN` удерживали по одному `File` на весь срок жизни итератора. Теперь они читают следующие блоки через тот же bounded LRU, что и GET. Аналогичный путь используется для чтения входных SST во время compaction. Форматы данных на диске не меняются.

## Изменения

- `SstEntryCursor::open_pooled` валидирует header, footer, index, Bloom filter, properties и первый data block SST, после чего закрывает отдельный исходный `File`. При переходе к очередному data block получает дескриптор через общий `Arc<TableHandlePool>`.
- `Database::scan` теперь подключает streaming cursors к тому же LRU, что и point `GET`. Открытые итераторы продолжают удерживать исходное поколение `VersionSet`, поэтому закрытие File в LRU не разрешает преждевременное удаление SST.
- Compaction читает входные SST через `table_scan::read_entries_pooled`: вместо `fs::read` всего файла используется потоковый cursor с ограничением числа resident SST handles.
- При повторном открытии pooled File проверяются физический размер, идентификатор базы, номер SST, диапазон sequence numbers и контрольная сумма заголовка. CRC32C каждого data block и прежние инварианты чтения сохранены.
- `table_handle_stats()` для `Database` и `ColumnFamilies` отражает resident read pool: `capacity`, `open_handles`, `hits`, `misses`, `evictions`, `reopens`.
- После удаления доказанно retired SST его номер очищается из `seen` кэша; нет бесконечного накопления IDs уже удалённых таблиц.

## Контракт для файловых дескрипторов

`Options::table_cache_capacity` ограничивает одновременно открытые **resident File внутри общего SST read pool** одного `Database`. Это относится к `GET`, следующим блокам `SCAN` и входным SST compaction. При переполнении LRU сначала закрывает прежний дескриптор, затем открывает новый. Пул сериализует операции mutex, сохраняя строгую квоту для собственной резидентности.

**Ограничение:** это не process-wide OS FD limit. Начальная валидация каждого cursor кратковременно открывает отдельный File вне pool, а создание SST outputs, WAL, MANIFEST, LOCK, deep verify и другие базы/именованные семьи используют отдельные descriptors. В момент проверки нового cursor все открытые FD процесса могут превышать `table_cache_capacity`; статистика показывает только resident pool. Общая задача #31 остаётся открытой.

Новый доступ к файлам не означает автоматического улучшения throughput, latency или RSS: shared mutex может ограничить параллелизм чтений. Для выводов о производительности нужны отдельные воспроизводимые benchmarks.

## Регрессионные тесты

- 48 SST, 480 записей и `table_cache_capacity=1`: проверены все значения, сортировка, LRU eviction, повторное открытие ранее вытесненного SST и reopening базы.
- Четыре параллельных `GET` worker и потоковый `SCAN` одновременно используют один LRU с capacity 1.
- Compaction с несколькими многоблочными SST inputs использует общий pool; активный старый iterator сохраняет право на чтение, а очистка удаляет retired SST только после release.
- Добавлен отдельный ручной тест на 1024 SST. Он помечен `#[ignore]` и не считается пройденным в стандартном CI.
- Сохранены прежние тесты recovery, MVCC transactions, FQL, named Column Families и checksum.

Функциональный PR [#43](https://github.com/Onmaynec/ForgeDB/pull/43), head `771f9a0feaf916e2ac9d53544364b60bf5bd6fb7`. [CI #43560292687](https://github.com/Onmaynec/ForgeDB/actions/runs/43560292687) завершён успешно: Ubuntu stable, Windows stable и Rust 1.89.

## Совместимость

Форматы IDENTITY V1, WAL V1, Internal Key V1, SSTable V1, MANIFEST reader V1–V2 / writer V2, CURRENT V1 и CF_REGISTRY V1 не меняются. Базы из v0.8.1 не требуют миграции. Команды `forgedb fql`, `forgedb shell`, `forgedb cf` и остальные CLI продолжают работать.

Компактация по-прежнему формирует bounded набор entries для merge: поддержка общего дескрипторного пула не является новым алгоритмом потоковой записи. Никаких mixed-family atomic transactions или глобального FD budget для нескольких открытых child DB в этой версии нет.

## Сборки и загрузки

В GitHub Pre-release публикуются готовые x86-64 CLI:

- `forgedb-v0.8.2-windows-x86_64.zip` — `forgedb.exe`;
- `forgedb-v0.8.2-linux-x86_64.tar.gz` — `forgedb`.

Для каждого архива публикуется отдельная `.sha256` с контрольной суммой. Дополнительный Rust toolchain для запуска не нужен. Подробности установки и проверки: `docs/QUICKSTART.md`.

## Документация

- `docs/releases/V0.8.2.md` — scope и проверки версии;
- `docs/adr/0018-streaming-sst-handle-leases.md` — lifetime cursors и ресурсные границы;
- `docs/CACHE_AND_PERFORMANCE.md`, `docs/OPTIONS.md`, `docs/INVARIANTS.md`;
- `CHANGELOG.md`.
