# ADR 0012: Indexed SSTable point reads и ограниченный decoded-block cache

Статус: принято для ForgeDB v0.6.0 (после release gate).

## Контекст

В v0.5.0 point read и transaction conflict validation вызывали `table_scan::read_entries`. Метод проверял SSTable, но каждый раз заново читал и декодировал все entries. Уже существующий `TableReader` имел проверенный index и Bloom filter, однако MVCC read path их не использовал.

## Решение

- Использовать SSTable index + Bloom filter для выбора data blocks, содержащих искомый ключ.
- Продолжать чтение соседних blocks, если несколько MVCC-версий одного user key находятся по разные стороны физической границы.
- Для L1+ использовать бинарный выбор кандидата в непересекающихся ranges.
- Ввести per-open `BlockCache` с бюджетом `Options::block_cache_size` для успешно декодированных blocks. При miss выполнить проверенное декодирование, а при превышении лимита вытеснить LRU.
- Для transaction conflict искать newest sequence тем же индексным путём, не сравнивая bytes значений.
- Сохранить independent full-decode path для `verify(deep)` и compaction.

## Отклонённые подходы

- Кэшировать только последний user key: не решает повторяющееся декодирование blocks.
- Доверять кэшированным байтам без проверки при miss: небезопасно для persisted corruption.
- Создать второй WAL или transaction protocol ради скорости: не связано с задачей.
- Объявлять `block_cache_size` абсолютным RAM limit: это неверно, поскольку текущий TableReader хранит исходные SSTable bytes отдельно.

## Последствия и границы

Код path меняется, persisted layouts не меняются. Верификация checksum становится не слабее, чем в v0.5.0. В default read path могут уменьшиться повторные CPU-декодирования, но ускорение не заявляется без benchmark; дорогостоящая eager загрузка всего физического SSTable в `TableReader::open` пока сохраняется.

Проверяем: snapshots/tombstones, block split одного ключа, conflict после flush, cache eviction, no-cache режим, Linux/Windows/Rust 1.89.
