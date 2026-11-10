# Путь чтения ForgeDB v0.4.0

## Источники

`Database::get` и `get_with_options` ищут последнюю видимую версию ключа. Внутренние ключи упорядочены по column family, user key, sequence по убыванию и value type.

Порядок поиска:

1. mutable MVCC memtable;
2. pending immutable memtable во время или после WAL rotation;
3. L0 SSTables, от новых к старым;
4. L1+ SSTables из `VersionSet`, с non-overlapping key ranges внутри каждого уровня.

Tombstone возвращает отсутствие значения и скрывает все более старые версии. Отсутствие ключа отличается от tombstone только внутри движка.

## Snapshots

`Database::snapshot()` фиксирует последний опубликованный sequence и регистрирует lease в `SnapshotRegistry`. При чтении с `ReadOptions::from_snapshot` версии с большим sequence невидимы. Snapshot из другого open handle или другой базы не принимается.

Привязка snapshot к активному registry необходима для retention при compaction; детали: [SNAPSHOTS_AND_MVCC.md](SNAPSHOTS_AND_MVCC.md).

## Range scan

Публичный `Database::scan(range, read_options)` создаёт forward merge iterator с границами и limit:

- memtable entries подготавливаются как in-memory cursors;
- SSTable `SstEntryCursor` удерживает индекс и один текущий data block;
- iterator объединяет источники по internal-key ordering, учитывает snapshot, tombstones и bounds;
- `Arc<VersionSet>` удерживает выбранное поколение файлов на протяжении чтения.

Range scan не загружает все SSTables целиком, но memtable entries в v0.4.0 материализуются при создании iterator. Обратного iterator пока нет.

## Проверка и ошибки

Перед возвратом данных проверяются ограничения ключей, принадлежность snapshot и применимые persisted-форматы. Проверка checksums управляется `ReadOptions`. Повреждённый live SSTable не маскируется чтением других файлов.

Read-only open получает эксклюзивный LOCK, выполняет recovery без изменения файлов и отклоняет write/flush/compaction. Несколько процессов не открывают один каталог одновременно.

Подробности физического размещения: [COMPACTION.md](COMPACTION.md) и [FILE_INVENTORY_AND_VERIFY.md](FILE_INVENTORY_AND_VERIFY.md).
