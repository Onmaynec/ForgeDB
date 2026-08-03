# File inventory и verify ForgeDB v0.3

## Назначение

Recovery и диагностика не должны удалять файлы только потому, что их назначение не удалось доказать. `FileInventory` классифицирует содержимое каталога без destructive cleanup.

## Категории

- `live_metadata`: `IDENTITY`, `CURRENT`, `LOCK`, active MANIFEST и служебные каталоги;
- `live_wal`: current и optional previous WAL из MANIFEST;
- `live_sst`: SSTables, на которые ссылается текущий VersionSet;
- `obsolete`: старые canonical WAL или MANIFEST;
- `orphan_sst`: canonical SSTable, отсутствующий в VersionSet;
- `temporary`: `TEMP` и объекты с suffix `.tmp`;
- `unknown`: всё, что не соответствует известному canonical имени;
- `blockers`: отсутствующие required live files.

Inventory не перемещает и не удаляет объекты.

## Recovery policy

Открытие блокируется, если отсутствует required authoritative object:

- `IDENTITY`;
- `CURRENT`;
- active MANIFEST;
- current WAL;
- previous WAL, когда он указан MANIFEST;
- любой live SSTable.

Orphan, obsolete, temporary и unknown objects не становятся live автоматически. Они отражаются в report и требуют отдельного доказуемого cleanup protocol.

## Quick verify

Quick verify выполняет:

- проверку VersionSet invariants;
- проверку file inventory;
- проверку наличия всех live objects;
- подсчёт уровней и live SSTables;
- отчёт об oldest active snapshot;
- warnings по orphan, temporary, obsolete и unknown objects.

Quick verify не перечитывает каждый data block.

## Deep verify

Deep verify дополнительно открывает каждый live SSTable в read order и повторно выполняет bounded decoding:

- header/footer и версии;
- CRC32C;
- block handles;
- index ordering;
- restart arrays;
- internal-key ordering;
- tombstone constraints;
- metadata consistency.

`entries_checked` содержит число полностью декодированных internal entries.

## Checksum option

`VerifyOptions::verify_checksums=false` может использоваться только для диагностического разделения структурной и checksum ошибки. Это не исправляет corruption и не делает повреждённый файл допустимым.

Обычный рекомендуемый режим — checksum verification enabled.

## Безопасность cleanup

Автоматический cleanup не входит в `v0.3.0`. Будущая реализация обязана:

1. зафиксировать authoritative VersionSet;
2. доказать отсутствие ссылки из current/previous metadata;
3. переместить объект в quarantine через atomic rename;
4. синхронизировать каталог;
5. удалить объект только отдельным шагом;
6. сохранить диагностическую запись операции.
