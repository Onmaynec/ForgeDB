# Политика выпусков ForgeDB

ForgeDB выпускается по milestone и использует Semantic Versioning.

## Pre-release до v1.0

Все версии `v0.x.y` публикуются как GitHub **Pre-release** с отдельным аннотированным Git tag. Stable release
появится только после выполнения критериев `v1.0.0`.

## Release gate

Для выпуска обязательны:

1. Чёткая и reviewable граница milestone.
2. Обновлённые русские `CHANGELOG.md` и `RELEASE_NOTES.md`.
3. Успешный `cargo fmt --all --check`.
4. Успешный Clippy для workspace/all targets/all features с `-D warnings`.
5. Успешные workspace tests, doctests и release build на Linux и Windows.
6. Успешные Clippy и tests на минимальной поддерживаемой версии Rust.
7. Явная документация crash consistency, recovery и persisted-format impact.
8. Process crash/corruption/model tests, соответствующие milestone.
9. Аннотированный Git tag, совпадающий с workspace version.
10. GitHub Release или Pre-release, созданный из reviewable release notes.

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
- tag и release создаются только после повторного полного validation gate;
- release-trigger основан на merged PR с точным release title.

## Нумерация

- patch: исправления без нового milestone и без несовместимого persisted layout;
- minor до v1.0: следующий roadmap milestone, возможны API changes;
- major: несовместимые stable API/format changes после v1.0.
