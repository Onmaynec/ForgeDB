# Snapshots и MVCC ForgeDB v0.3

## Модель sequence

Каждая операция `WriteBatch` получает непрерывный диапазон глобальных ненулевых sequence numbers. Более новый sequence всегда логически новее более старого.

Internal-key comparator сортирует:

1. column family по возрастанию;
2. user key по возрастанию;
3. sequence по убыванию;
4. value type по убыванию.

Поэтому версии одного user key находятся рядом, а первая версия с `sequence <= snapshot_sequence` является видимой.

## Snapshot handle

`Database::snapshot()` фиксирует последний опубликованный sequence и возвращает дешёвый immutable handle.

Snapshot содержит:

- persisted `DatabaseId`;
- случайный идентификатор конкретного open handle;
- optional sequence;
- shared lease в registry active snapshots.

Клонирование snapshot клонирует `Arc` lease и не создаёт новую запись registry. Запись удаляется после уничтожения последнего clone.

Snapshot другой базы или другого open handle отклоняется как `InvalidArgument`. После закрытия и повторного открытия базы старый snapshot нельзя использовать в новом handle.

## Пустая база

Snapshot, созданный до первой записи, имеет `sequence = None`. Он всегда видит пустое состояние, даже если после его создания появились записи.

## Visibility

Для user key видима первая версия, удовлетворяющая условиям:

- версия относится к тому же user key;
- `version.sequence <= snapshot.sequence`;
- версия находится в наиболее приоритетном read source, содержащем видимое состояние.

`Value` возвращает bytes. `Deletion` скрывает все более старые значения.

## Flush

Flush не изменяет snapshot semantics:

- до rotation версии находятся в mutable memtable;
- после rotation они находятся в immutable memtable;
- после installation те же internal keys находятся в L0 SSTable.

Snapshot, созданный до overwrite или delete, продолжает видеть старую версию после flush, пока соответствующая версия физически сохранена.

## Oldest active snapshot

Registry предоставляет `oldest_active_sequence`. В `v0.3.0` значение используется в verify report и подготавливает контракт для compaction.

Будущая compaction не сможет удалять version/tombstone, если он нужен хотя бы одному active snapshot. Политика физического удаления появится в milestone `v0.4.0` и должна использовать одновременно snapshot horizon и нижележащие уровни.

## Ограничения v0.3.0

- Snapshot существует только в памяти процесса и не сериализуется.
- Snapshot не переносится между open handles.
- Нет named snapshots и backup snapshots.
- Нет transaction snapshot isolation.
- Пока compaction отсутствует, версии не удаляются из SSTables.
