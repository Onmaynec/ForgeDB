# ADR 0014: Lazy random-access SSTable readers

- Статус: принято для v0.6.2
- Дата: 2026-11-14

## Контекст

В v0.6.0 индексный point lookup перестал декодировать весь SSTable при каждом `get`. Но `TableReader::open` по-прежнему полностью загружал raw SST bytes в `Vec<u8>` для каждого live файла из `VersionSet`. Ограничение `Options::block_cache_size` относилось только к decoded data blocks и не управляло этим raw буфером.

Число live SST увеличивается при росте базы, что приводит к расходу RAM, пропорциональному общей сумме физических размеров файлов.

## Решение

`TableReader` теперь хранит `Arc<Mutex<Box<dyn RandomAccessReader>>>`. При открытии он проверяет физический размер из MANIFEST, читает фиксированные header/footer, а также проверенные blocks фильтра, индекса и свойств через random-access интерфейс `FileSystem`.

Data blocks не загружаются при open. `get`, `get_at_cached` и `latest_sequence_after_cached` получают только нужный block по `BlockHandle`, повторяют проверки границ, compression, reserved bytes и CRC32C, после чего декодируют записи. Блоки, помещённые в `BlockCache`, сохраняют прежнюю политику LRU.

Открытый handle удерживается до уничтожения последнего `TableReader`. Его `Arc` доступен старым `VersionSet` и итераторам; `Weak<TableReader>` в retirement queue не позволит удалить файл, пока reader жив. Синхронизация `seek/read` происходит одним mutex на reader, без `unsafe` и платформенных read APIs.

## Альтернативы

- Eager physical SST buffering: проще, но не ограничивает RAM при росте live SST.
- Открывать `File` для каждого чтения: больше системных вызовов, усложнение lifetime protection и TOCTOU с замещением файлов.
- mmap: отдельные требования к safety/Windows и `unsafe`, не согласуется с политикой workspace.

## Последствия

- RAW SST bytes не сохраняются в памяти reader, но index/Bloom/properties, временные I/O buffers, decoded cache и memtables занимают память отдельно.
- `Options::table_cache_capacity` пока не ограничивает количество открытых файлов. Один `TableReader` держит один handle на live SST. Нужна будущая политика eviction без изменения гарантий reference-aware cleanup.
- Чтения одного reader сериализуются по `seek/read`. Для оценки contention необходимы одинаковые benchmark workloads на обеих версиях; числовых гарантий ускорения нет.
- Reader проверяет служебные блоки при open и data blocks при обращении, не весь контент при open. `verify --deep` остаётся полным независимым проходом. Уже проверенный decoded block в кэше не перечитывает физические байты на каждом hit.

## Persisted format и recovery

Новых форматов или полей нет: WAL V1, SSTable V1, MANIFEST writer V2/reader V1-V2, CURRENT V1. Протоколы file creation/sync/rename и MANIFEST installation не меняются. Cross-restart GC не вводится и orphan files не удаляются автоматически.

## Проверка

- Instrumented `FileSystem`: measured bytes at open меньше размера физического SST, data read увеличивает счётчик, cache hit не запускает нового I/O.
- Existing checksum/future-version/snapshot-split-key/transaction-conflict/reopen/iterator-lifetime regression suite.
- Ubuntu, Windows и Rust 1.89 с `-D warnings`.
