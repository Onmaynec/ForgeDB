# Политика выпусков ForgeDB

ForgeDB выпускается по milestone и использует Semantic Versioning.

## Стабильный релиз v1.0.0

Все выпуски `v0.x.y` были GitHub **Pre-release**. Начиная с v1.0.0, публичный Stable Release означает документированный API, обратную совместимость прежних persisted форматов и проверенные Windows/Linux CLI. Это **не** означает выпуск в crates.io: имя `forgedb` занято другим проектом (Issue #27).

## Release gate

Для выпуска обязательны:

1. Чёткая и reviewable граница milestone.
2. Обновлённые русские `CHANGELOG.md` и `RELEASE_NOTES.md`.
3. Успешный `cargo fmt --all --check`.
4. Успешный Clippy для workspace/all targets/all features с `-D warnings`.
5. Успешные workspace tests, doctests, cargo doc и release build на Linux и Windows.
6. Успешные Clippy и tests на минимальной поддерживаемой версии Rust.
7. Явная документация crash consistency, recovery и persisted-format impact.
8. Process crash/corruption/model tests, соответствующие milestone.
9. Аннотированный Git tag, совпадающий с workspace version.
10. GitHub Release или Pre-release, созданный из reviewable release notes.
11. В stable v1.0.0: после Linux validation обязательны отдельные Windows/Linux CLI binary smoke (put/doctor/export/restore/get), проверки SHA-256, и только затем перевод GitHub Release из Draft в публичный Stable.
12. Известные ограничения (`#31`, `#35`, `#27`) остаются открытыми, пока не реализованы.

Windows и MSRV — обязательные release checks, а не информационные сигналы.

## Persisted-форматы

Изменение persisted format требует до публикации:

- новой версии формата;
- ADR;
- точного layout;
- compatibility fixtures;
- rejection tests для unknown future version;
- migration и rollback policy;
- обновления recovery/durability документов.

## Автоматизация

Release workflow идемпотентен:

- существующий tag не перемещается и не перезаписывается;
- существующий GitHub Release не заменяется;
- annotated tag создаётся после повторного validation gate и не перемещается при повторном запуске;
- v1.0.0 запускается по push в main с заголовком `release: v1.0.0` либо явному workflow_dispatch на main; дублирующий pull_request.closed trigger убран;
- оба бинарника собираются из immutable tagged commit, end-to-end тестируются и проходят проверку контрольных сумм;
- стабильный выпуск создаётся сначала как **Draft** и становится публичным только после подтверждения всех четырёх вложений и статуса `uploaded`.

## Нумерация

- patch: исправления без нового milestone и без несовместимого persisted layout;
- minor после v1.0: обратно совместимые расширения API, версионированные storage changes;
- major после v1.0: намеренно несовместимые stable API/format изменения с ADR, migration и rollback.
