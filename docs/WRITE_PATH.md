# Путь записи ForgeDB v0.6.0

## Публичные операции

`Database::put`, `delete` и `write` используют единый путь `WriteBatch`. Перед записью проверяются открытое состояние базы, режим read-only, ключи, значения, число операций, размер batch и ограничения памяти.

Операции одного batch получают непересекающийся непрерывный sequence range. Последняя операция с одним ключом побеждает внутри batch. Декодирование WAL в recovery применяет batch целиком.

## Порядок `Database::write`

1. Вызов публичного wrapper проверяет L0 write pressure; при необходимости выполняет compaction или ограниченную задержку.
2. Под mutex проверяется отсутствие fatal storage error.
3. Назначаются sequence start/end и проверяется переполнение.
4. Mutable memtable выполняет preflight без изменения видимого состояния.
5. Batch кодируется и добавляется в один WAL V1 frame, если WAL включён.
6. При `Sync` выполняется WAL `sync_all` до публикации в memtable.
7. Под тем же mutex staged changes полностью применяются к memtable и публикуется новый sequence.

`NoSync` не требует синхронизации WAL при каждом вызове; `DisableWal` пропускает журнал. Их crash durability отличается от `Sync`: [DURABILITY.md](DURABILITY.md).

## Ошибки записи

Validation/preflight ошибки до WAL не изменяют состояние базы. Ошибка WAL append/sync не публикует batch и переводит writer в fatal state: продолжать запись при неопределённом результате I/O нельзя.

Public write path при переполнении L0 пытается снизить pressure через synchronous compaction; если stop threshold по-прежнему превышен, возвращается `Busy`. Читатели продолжают работать с выбранным immutable `VersionSet`.

## Flush

`Database::flush()`:

1. синхронизирует current WAL;
2. резервирует SST/WAL generation и переводит mutable записи в pending immutable memtable;
3. создаёт новый WAL и фиксирует смену поколений в MANIFEST;
4. строит SSTable с проверенными блоками и синхронизирует файл/каталог до публикации;
5. устанавливает SSTable в L0 через durable MANIFEST edit;
6. только после установки может удалить завершённый WAL generation.

Повторный crash при незавершённом flush восстанавливает previous/current WAL раздельно. Физический SST не становится live только из-за существования в каталоге.

## Leveled compaction

После публичного `flush` wrapper запускает synchronous compaction при достижении trigger. Также доступен `Database::compact_range` с half-open range.

Compaction резервирует output file numbers durable edit, строит все outputs и устанавливает их одним MANIFEST V2 edit. Версии, необходимые активным snapshots, сохраняются. Старые input SST переходят в process-local retirement queue только после synced installation edit; manual `cleanup_obsolete_sstables()` удаляет их после освобождения читателей. При restart не предпринимается speculative orphan deletion.

Полный порядок installation и retention: [COMPACTION.md](COMPACTION.md).

## Close

`close()` синхронизирует WAL/MANIFEST writers и освобождает LOCK только после успешной операции. Закрытие во время активного maintenance job возвращает `Busy`.

## Отсутствует в v0.6.0

Group commit, background compaction scheduler, range tombstones и полная automatic/cross-restart garbage collection SST. Optimistic snapshot-isolation transactions существуют с v0.5.0. Индексный point read и bounded decoded-block cache — с v0.6.0.
