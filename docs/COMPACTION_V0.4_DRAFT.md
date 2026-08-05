# Leveled compaction ForgeDB v0.4 — draft

Статус: development contract для draft PR v0.4.0. Документ не является обещанием выпущенной версии.

## Selection

ForgeDB использует отдельные deterministic picker-ы:

- background picker выбирает highest eligible score;
- manual range picker выбирает первый source SST, пересекающий half-open range, независимо от background pressure в других диапазонах.

L0→L1 строит overlap closure по L0 и L1. Ln→Ln+1 выбирает source file и все target overlaps. Любой plan ограничен `max_compaction_bytes`.

Public `compact_range` сначала flush-ит mutable state и фиксирует set исходных source SST, пересекающих range. Затем выполняет durable installation steps, пока каждый исходный file number не исчезнет из source levels. Newly created outputs не добавляются в этот work set и могут быть продвинуты следующим manual вызовом.

После каждого step сравнивается source layout. Отсутствие progress при непустом work set является internal invariant error. Один вызов ограничен 10 000 steps и возвращает `ResourceLimit`, если safety bound исчерпан.

## Merge и outputs

Input SST читаются block-at-a-time с обязательной checksum verification. Merge сохраняет версии, необходимые oldest active snapshot, и удаляет tombstones только по bottommost rules. Output splitter не разделяет версии одного logical key.

Output SST полностью записываются, синхронизируются и публикуются до финального MANIFEST V2 edit. Финальный edit атомарно удаляет exact inputs, добавляет outputs и обновляет compaction pointer. Current immutable VersionSet меняется только после sync MANIFEST.

## Obsolete lifetime

Старая Version удерживается readers через `Arc<VersionSet>`. Очередь obsolete files хранит только `Weak<VersionSet>`. Input SST нельзя удалить, пока Version удерживает reader, iterator или другой operation. `file_inventory()` не изменяет каталог; cleanup выполняется только mutating paths.

Crash до cleanup может оставить безопасный orphan SST. Он не включается в live state автоматически.

## Write stalls

При L0 slowdown threshold write path выполняет bounded delay вне database state lock. При stop threshold write возвращает `Error::Busy`; reads и manual compaction остаются доступны.

## Validation status

Picker, merge, output splitting, MANIFEST V2 installation, snapshot preservation, reference-aware obsolete lifetime, write stalls, requested-range selection и bounded exhaustive original-source processing подтверждены clean read-only CI на Ubuntu stable, Windows stable и Rust 1.89.

Installation failpoint/crash matrix, background scheduler и stall metrics остаются незавершёнными slices draft milestone.
