# Leveled compaction ForgeDB v0.4 — draft

Статус: development contract для draft PR v0.4.0. Документ не является обещанием выпущенной версии.

## Уже подтверждённый путь

ForgeDB использует отдельные deterministic picker-ы:

- background picker выбирает highest eligible score;
- manual range picker выбирает первый source SST, пересекающий half-open range, независимо от background pressure в других диапазонах.

L0→L1 строит overlap closure по L0 и L1. Ln→Ln+1 выбирает source file и все target overlaps. Любой plan ограничен `max_compaction_bytes`.

Input SST читаются block-at-a-time с обязательной checksum verification. Merge сохраняет версии, необходимые oldest active snapshot, и удаляет tombstones только по bottommost rules. Output splitter не разделяет версии одного logical key.

Output SST полностью записываются, синхронизируются и публикуются до финального MANIFEST V2 edit. Финальный edit атомарно удаляет exact inputs, добавляет outputs и обновляет compaction pointer. Current immutable VersionSet меняется только после sync MANIFEST.

## Obsolete lifetime

Старая Version удерживается readers через `Arc<VersionSet>`. Очередь obsolete files хранит только `Weak<VersionSet>`. Input SST нельзя удалить, пока Version удерживает reader, iterator или другой operation. `file_inventory()` не изменяет каталог; cleanup выполняется только mutating paths.

Crash до cleanup может оставить безопасный orphan SST. Он не включается в live state автоматически.

## Write stalls

При L0 slowdown threshold write path выполняет bounded delay вне database state lock. При stop threshold write возвращает `Error::Busy`; reads и manual compaction остаются доступны.

## Текущий validation status

Один manual installation step и requested-range picker подтверждены Ubuntu stable, Windows stable и Rust 1.89. Bounded exhaustive loop по всем пересекающимся plans находится в следующем validation slice и не считается подтверждённым до отдельного clean read-only CI run.
