# Измерение производительности ForgeDB v0.9.0

## Что проверяем

В v0.9.0 оптимизируются три независимых горячих пути:
- native SST file-handle LRU: eviction O(log N) вместо O(N) по количеству открытых File;
- `DbIterator`: меньше копирований больших values при пропуске MVCC версий;
- `VersionSet::latest_sequence_after`: бинарный поиск кандидата для L1+ вместо перебора непересекающихся SST.

Улучшенная Big-O оценка не гарантирует меньшую задержку: lock contention, cache locality, filesystem cache и стоимость дополнительного BTreeSet могут менять результат. Достоверный вывод требует одинаковой среды и множественных прогонов.

## Воспроизводимый benchmark

```powershell
# Windows PowerShell, установлен Rust 1.89+:
cargo run --locked --release -p forgedb --example performance_bench -- 8192 32768
```

```bash
# Linux:
cargo run --locked --release -p forgedb --example performance_bench -- 8192 32768
```

Параметры: **8192 user keys**, **32768 GET за каждый проход**, value **256 байт**, `block_cache_size=0`, `table_cache_capacity=8`, data block 1024 байта. Данные пишутся группами по 128 ключей с flush после каждой группы: фиксированное распределение SST и детерминированный выбор ключей `index = (iteration * 7919) % keys`.

Benchmark выполняет: setup, cold GET, warm GET, полный `SCAN` и проверку количества/суммы байт. Печатает `elapsed_ms`, GET `ops_sec`, `p50_ns/p95_ns/p99_ns`, SCAN `entries_per_sec` и `TableHandleStats` (capacity/open_handles/hits/misses/evictions/reopens). Данные удаляются после закрытия базы. В тесте используется `black_box` результатов, чтобы компилятор не устранял необходимые вычисления.

CI выполняет два режима:

```bash
# Ubuntu и Windows — быстрая проверка корректности:
cargo run --locked --release -p forgedb --example performance_bench -- 256 1024

# Дополнительно Ubuntu — полный workload с реальным eviction:
cargo run --locked --release -p forgedb --example performance_bench -- 8192 32768
```

При полном workload проверяется, что `table_handle_stats().evictions > 0` при числе созданных SST больше восьми. Это подтверждает, что benchmark реально проходит через индексированный LRU, но само по себе **не** даёт статистически значимого сравнения производительности или SLA.

## Сравнение версий

1. Сравнивать v0.8.2 и v0.9.0 при одинаковых аргументах, ОС, файловой системе и toolchain. Старую версию можно измерять через `point_lookup_bench`, но его конфигурация и профиль запросов **не идентичны** новому `performance_bench`, поэтому напрямую делить полученные `ops/sec` некорректно. Для честного сравнения запускать один и тот же benchmark workload на обеих версиях.
2. На каждую версию выполнить минимум 5 прогонов; чередовать порядок версий, чтобы избежать систематического влияния нагрева и OS page cache.
3. Отдельно анализировать GET p50/p95/p99, SCAN entries/sec, число SST evictions/reopens и setup duration. Среднее значение может скрывать long-tail latency; использовать медиану нескольких прогонов.
4. Измерять на рабочем наборе с большим числом SST и малой `table_cache_capacity`, иначе индекс LRU может не применяться. С `8192` ключами по 128 в SST создаётся около 64 SST: при capacity=8 случайный по фиксированному шагу GET-workload вызывает вытеснения. CI smoke с 256 ключами создаёт только две SST, поэтому eviction в нём не ожидается. При cache hits decoded-block чтения из файлов вообще пропускаются, поэтому выбор benchmark parameters меняет охват.
5. В release notes не записывать процент «ускорения» без воспроизводимого A/B; CI smoke предназначен для проверки отсутствия падений.

## Гарантии

Тесты `performance_regression.rs` проверяют корректность LRU, snapshot и optimistic conflict после compaction. Они **не** являются benchmarks; их время выполнения не применяется как доказательство performance gain. Система не обещает bounded process-wide FD, общего RSS budget или ускорения при произвольной конкурентной нагрузке.
