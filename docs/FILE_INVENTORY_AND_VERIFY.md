# File inventory, recovery plan и verify ForgeDB v0.3

## Назначение

Recovery и диагностика не должны удалять файлы только потому, что их назначение не удалось доказать. `FileInventory` классифицирует содержимое каталога без destructive cleanup, а `RecoveryPlan` показывает authoritative state и только предлагаемые действия.

## Категории

- `live_metadata`: `IDENTITY`, `CURRENT`, `LOCK`, active MANIFEST и служебные каталоги;
- `live_wal`: current и optional previous WAL из MANIFEST;
- `live_sst`: SSTables, на которые ссылается текущий VersionSet;
- `obsolete`: старые canonical WAL или MANIFEST;
- `orphan_sst`: canonical SSTable, отсутствующий в VersionSet;
- `temporary`: `TEMP` и объекты с suffix `.tmp`;
- `backup_artifacts`: имена `BACKUP-*`, а также suffix `.bak`, `.backup`, `.old` или `~`;
- `user_files`: только явно выделенный namespace `.forgedb-user` и имена `USER-*`;
- `unknown`: всё, что не соответствует известной категории;
- `blockers`: отсутствующие required live files.

Явная политика имён для backup и user files нужна, чтобы ForgeDB не угадывала назначение произвольного объекта. Обычный `notes.txt` остаётся `unknown`, а не автоматически считается пользовательским файлом.

Inventory не перемещает и не удаляет объекты.

## Recovery plan

Нормативная команда:

```bash
forgedb inspect recovery-plan ./database
```

`RecoveryPlan::inspect`:

1. получает process lock каталога;
2. читает и проверяет `IDENTITY`;
3. выбирает MANIFEST через `CURRENT`;
4. восстанавливает immutable `VersionSet`;
5. определяет WAL replay order: optional previous, затем current;
6. строит file inventory;
7. формирует warnings, blockers и предложения safe cleanup;
8. освобождает lock без изменения базы.

Отчёт содержит:

- путь базы;
- выбранный MANIFEST;
- WALs для replay в правильном порядке;
- live SSTables;
- obsolete, orphan, temporary, backup, user и unknown files;
- planned safe cleanup;
- warnings;
- blockers;
- признак возможности normal open.

`planned_safe_cleanup` является только планом. Команда не выполняет rename, quarantine или delete.

## Recovery policy

Открытие блокируется, если отсутствует required authoritative object:

- `IDENTITY`;
- `CURRENT`;
- active MANIFEST;
- current WAL;
- previous WAL, когда он указан MANIFEST;
- любой live SSTable.

Orphan, obsolete, temporary, backup, user и unknown objects не становятся live автоматически. Unknown и user files никогда не включаются в автоматический cleanup.

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

Начиная с v0.6.0 `Database::cleanup_obsolete_sstables()` удаляет **только process-local proven retired SSTable inputs** после synced MANIFEST compaction installation и освобождения конкретного `TableReader`, включая старые итераторы. Результат содержит списки `removed`, `deferred`, `absent` и `failed`. Команда `forgedb compact` вызывает эту проверку после compaction.

Этот путь не пытается восстанавливать историю удаления по directory listing после restart. `FileInventory` и `RecoveryPlan` остаются неразрушающими и **не дают разрешения на удаление** `orphan_sst`, даже если файл имеет canonical SST name. Backup, user, unknown и произвольные temporary objects исключены из удаления.

Полноценная restart-aware garbage collection и карантин с persistent journal ещё не реализованы; соответствующие гарантии не заявляются. См. [CACHE_AND_PERFORMANCE.md](CACHE_AND_PERFORMANCE.md) и ADR 0013.
