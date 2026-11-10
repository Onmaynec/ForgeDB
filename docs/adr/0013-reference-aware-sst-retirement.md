# ADR 0013: Консервативное удаление retired SSTables

Статус: принято для ForgeDB v0.6.0 (после release gate).

## Контекст

После compaction старые input SST исключаются из MANIFEST, но физически оставались на диске. Удалять их сразу небезопасно: другой iterator или версия LSM может продолжать читать файл. На Windows открытый reader часто предотвращает unlink; один только `Weak<VersionSet>` недостаточен, поскольку поколения версий могут разделять один `Arc<TableReader>`.

## Решение

- Добавлять retired inputs в очередь только после successful durable MANIFEST compaction installation edit.
- Отслеживать каждый retired file своим `Weak<TableReader>`, полученным из исходного `TableHandle`.
- Пока strong reader существует, откладывать physical unlink. Iterator дополнительно удерживает `Arc<VersionSet>` до завершения scan.
- Отказывать в cleanup на read-only handle и во время flush/compaction.
- Проверять, что удаляемый номер отсутствует в current VersionSet; использовать canonical SST file name.
- Возвращать отчёт об удалённых, отложенных, отсутствовавших и failed файлах. Ошибки удаления не переписывают MANIFEST.
- Не удалять orphan SST, неизвестные файлы, backups, файлы пользователей и SST, полученные при открытии из неоднозначного состояния.

## Crash consistency

Успешная запись MANIFEST — единственная смена live topology. До установки input остаётся authoritative. После неё output становится authoritative; физическое удаление input не меняет логическую state. Ошибка очистки безопасна: файл останется orphan, но recovery продолжит использовать MANIFEST.

Очередь retirement process-local и не восстанавливается после restart. Полная persisted reference-aware garbage collection требует отдельного протокола и не заявляется для v0.6.0.

## Проверка

- Активный iterator читает прежнее поколение после compaction, cleanup откладывает unlink.
- После drop iterator cleanup удаляет доказанные obsolete inputs.
- Активный snapshot видит необходимую old version после compaction+cleanup.
- Orphan после рестарта остаётся на диске, read-only cleanup отклоняется.
- Windows, Linux, Rust 1.89 release gate.
