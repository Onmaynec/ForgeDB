# Восстановление ForgeDB v0.6.0

## Авторитетное состояние

`CURRENT` указывает на активный MANIFEST. `ManifestState` определяет current/previous WAL, номера файлов, последний sequence и live SSTables в L0/L1+. Существование SST на диске само по себе не делает его live.

При открытии база получает эксклюзивный LOCK, проверяет IDENTITY и persisted versions, выполняет strict MANIFEST replay и подтверждает наличие MANIFEST-owned WAL/SST. Missing live file считается corruption, а не причиной создать пустую замену.

## WAL recovery

После проверки MANIFEST-owned файлов `previous WAL` восстанавливается в pending immutable memtable, `current WAL` — в mutable memtable. Каждый полный WAL frame проходит length, field, sequence, UUID и CRC32C validation до применения batch.

Применение одного `WriteBatch` атомарно: либо все операции кадра проходят preflight и replay, либо ни одна не становится видима.

Только физически неполный последний frame допускается как trailing partial. Writable open усекает такой хвост до последней проверенной границы; read-only open ничего не изменяет. Mid-file corruption и checksum mismatch полного кадра запрещают дальнейший replay. Sequence ranges должны строго возрастать; gaps после `DisableWal` допустимы.

## MANIFEST V1 → V2

Writable open V1 сначала проверяет старое состояние, затем создаёт новый MANIFEST V2 snapshot под новым номером, синхронизирует файл и публикует `CURRENT`. Старый MANIFEST V1 не перезаписывается. Read-only open V1 не выполняет migration.

V1 reader не должен интерпретировать V2 как V1. Future versions отклоняются. Детали: [COMPATIBILITY.md](COMPATIBILITY.md) и [formats/MANIFEST_V2.md](formats/MANIFEST_V2.md).

## Flush и compaction crash boundaries

Flush резервирует номера и сохраняет отдельно current/previous WAL. До successful SST installation edit durable WAL продолжает быть источником recovery.

Для compaction:

1. reserved output numbers сохраняются до создания outputs;
2. outputs создаются и синхронизируются, но пока не считаются live;
3. один synced MANIFEST edit удаляет input references и добавляет output references;
4. новый `VersionSet` публикуется после durable installation.

Crash до installation сохраняет inputs authoritative; после него authoritative становятся outputs. Output SST без MANIFEST reference является orphan.

Неразмеченные, backup и user files не удаляются normal recovery. В v0.6.0 после synced compaction edit можно вручную вызвать `cleanup_obsolete_sstables()`: метод удаляет только retired inputs, доказанно исключённые из live topology в текущем open handle и больше не используемые `TableReader`. После restart неклассифицированные orphan SST остаются нетронутыми, потому что process-local retirement queue не восстанавливается автоматически.

## Диагностика

`file_inventory()` показывает live, obsolete, orphan, temporary и unknown файлы. `verify(VerifyOptions::quick())` проверяет метаданные и топологию, `deep()` дополнительно читает live SST entries/blocks. Проверка не изменяет базу.

CLI:

```bash
forgedb inventory ./db
forgedb verify ./db --deep
forgedb inspect recovery-plan ./db
```

Подробная классификация: [FILE_INVENTORY_AND_VERIFY.md](FILE_INVENTORY_AND_VERIFY.md). Salvage повреждённых баз пока отсутствует.

## Границы гарантий

Автоматические tests включают повторное открытие, проверки повреждений и process abort. Эти проверки не равнозначны подтверждённой устойчивости к произвольному power loss, неисправности накопителя или OS crash. Durability зависит от режима записи и поведения файловой системы: [DURABILITY.md](DURABILITY.md).
