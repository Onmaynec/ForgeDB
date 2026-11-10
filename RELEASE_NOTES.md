# ForgeDB v0.6.1 — Stability and Recovery

Статус: GitHub Pre-release.

Исправительный выпуск поверх v0.6.0. Основные изменения затрагивают закрытие базы, повторные публичные записи при заполненной memtable и автоматическую безопасную очистку SSTable после compaction. Persisted-форматы не меняются.

## Исправлено

- `Database::close()` больше не помечает базу закрытой и не извлекает WAL/MANIFEST writer handles до успешного sync. При ошибке остаётся возможность повторить закрытие; process LOCK не освобождается преждевременно.
- `lock_state()` проверяет `closed` после захвата mutex, чтобы не пропускать операции через гонку с успешным закрытием.
- Публичные `put`, `delete` и `write` при `ResourceLimit { resource: "memtable_memory", .. }` делают один synchronous flush/retry. Повторный отказ не записывает часть batch; бесконечных повторов нет.
- После успешной leveled compaction вызывается best-effort reference-aware очистка retired SSTables. Удерживаемые читателями файлы не удаляются; неудачный unlink не превращает уже завершённую compaction в ошибку записи.
- Удалены неиспользуемые источники pre-MVCC storage core; уточнены документы, комментарии API и активные/зарезервированные настройки `Options`.

## Гарантии и ограничения

- `Sync` сохраняет прежнюю WAL durability boundary; не добавлено новых persisted records.
- WAL V1, SSTable V1, Internal Key V1 и CURRENT V1 без изменений; MANIFEST writer V2, reader V1/V2.
- Auto-flush и compaction остаются **синхронными**: отдельного worker pool нет. Транзакционный commit не выполняет новый automatic flush/retry.
- Retired-file cleanup действует только в текущем открытом процессе; произвольные orphan SST после перезапуска не удаляются.
- Параметры `table_cache_capacity`, `max_background_flushes` и `max_background_compactions` зарезервированы для будущих изменений; их наличие не означает lazy reader eviction или фоновые jobs.
- `TableReader` по-прежнему eagerly загружает физические SST bytes, и decoded-block cache budget не равен лимиту всего RSS.
- Snapshot isolation не равна serializable isolation; FQL и полный backup API в эту версию не входят.

## Проверки

Feature head `bea0b52ed9ccabcd144b91ad55f7e6130453f2b8`, [CI run 42880868115](https://github.com/Onmaynec/ForgeDB/actions/runs/42880868115):

- Ubuntu stable: rustfmt, strict Clippy, tests, doctests, release build, benchmark smoke — success.
- Windows stable: тот же набор — success.
- Rust 1.89: strict Clippy, workspace tests — success.

В проверках есть deterministic fault injection для обеих sync boundaries `close()`, повторное закрытие и reopen, continuous writes при заполненной memtable, oversized atomic batch и проверка автоматического удаления SST с сохранением active iterator/snapshot и orphan файлов. Release workflow отдельно повторяет Linux validation до создания аннотированного тега.

## Готовый CLI

Release workflow собирает CLI `forgedb` для x86-64 Windows и Linux и прикрепляет архивы с SHA-256 checksum к GitHub Release:

- `forgedb-v0.6.1-windows-x86_64.zip` — `forgedb.exe`;
- `forgedb-v0.6.1-linux-x86_64.tar.gz` — `forgedb`.

После распаковки CLI можно запускать без установленного Rust toolchain:

```bash
forgedb put ./data hello world
forgedb get ./data hello
forgedb scan ./data
forgedb verify ./data --deep
```

CLI работает с UTF-8 текстовыми ключами/значениями, библиотечный API принимает бинарные данные. На Windows используйте `./forgedb.exe` или `.\forgedb.exe` в PowerShell.

## Документация

- `docs/releases/V0.6.1.md` — границы исправительного milestone;
- `docs/ARCHITECTURE.md` и `docs/WRITE_PATH.md` — lifecycle, flush/retry, compaction;
- `docs/CACHE_AND_PERFORMANCE.md` и `docs/COMPACTION.md` — reference-aware cleanup;
- `docs/OPTIONS.md` — действующие и зарезервированные параметры.
