# ADR 0015: Изолированные Column Families без смены SSTable/MANIFEST

- Статус: принято для v0.7.0
- Дата: 2026-11-14

## Контекст

`ColumnFamilyId` содержится в Internal Key V1/WAL operation encoding, но до v0.7 SSTable properties, key ranges и leveled compaction поддерживали только default CF. Прямое снятие проверки `WriteBatch::put_cf` привело бы к смешению одноимённых ключей на путях чтения, индекса и conflict validation. Полная реализация multi-family в одном LSM потребовала бы новый authoritative persisted MANIFEST и пересмотр metadata/range comparator.

## Выбор

В v0.7.0 каждый именованный CF является самостоятельной ForgeDB базой в зарезервированном подкаталоге `.forgedb-column-families/CF-<decimal-id>`. Старый root остаётся default CF без преобразования существующих данных. `ColumnFamilies` — фасад, который сериализует регистрацию имён, контролирует `open` child handles и предоставляет операции по ID.

Имена фиксируются отдельным durable `CF_REGISTRY V1`. В режиме создания новый child DATABASE полностью инициализируется до публикации его ID; при drop registry перестаёт ссылаться на ID, но физические файлы не удаляются. ID монотонно увеличивается, пути строятся по числовому ID, не по пользовательской строке.

## Следствия

1. Изоляция одноимённых user keys обеспечена отдельными WAL, memtable, SST, MANIFEST и последовательностями версий, не только фильтром в API.
2. Atomic WriteBatch и optimistic snapshot isolation относятся к одной family. Глобальных sequence numbers, cross-family snapshot, atomic mixed-family write/commit и глобального WAL пока нет. `WriteBatch::put_cf` с ненулевым ID остаётся закрытым для старого storage core; вместо него применяется `ColumnFamilies::write(id, batch)`.
3. Multi-family compaction и WAL recovery происходят независимо в отдельных каталогах. Process lock корневого DB охватывает сериализацию записи registry; у каждого child DB собственный lock. Чужие/незарегистрированные directory names не интерпретируются как families.
4. Drop не освобождает место автоматически. Без отдельного tombstone-aware reaper удалять orphan directories опасно, в том числе на Windows.
5. Фасад держит отдельные открытые child engines, поэтому общий ресурсный budget между families пока не задан; `block_cache_size` применяется к каждому открытому DB, а не глобально. Ограничение числа SST file handles из #31 остаётся отдельным.
6. Persisted format старого root не меняется. Старые клиенты, которые не знают CF registry, могут открыть root как default DB, но не получат доступ к именованным CF. Для корректного управления именами нужен новый API.

## Отложено

Возможный единый физический WAL/MANIFEST и mixed-family atomic batches потребуют форматных ADR и migration/compatibility strategy. Нельзя без перехода объявлять общий sequence/snapshot или делать автоматическую очистку orphan.

См. `docs/formats/CF_REGISTRY_V1.md`, `docs/COMPATIBILITY.md`, `docs/INVARIANTS.md`.
