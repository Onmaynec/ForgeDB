# ForgeDB v0.7.0 — Column Families

Статус: GitHub Pre-release.

ForgeDB v0.7.0 добавляет именованные Column Families с долговечным реестром имён/ID, изоляцией keyspaces, собственными LSM/WAL и удобным API. Старую default базу можно продолжить использовать без миграции. Новая функциональность доступна из Rust и CLI для Windows/Linux.

## Новое

### API Column Families

`ColumnFamilies::open(path, Options)` открывает старую default DB и проверяет CF registry. Именованные families доступны через:

- `list()`, `create(name)`, `drop_family(id)`, `close()`, `default_database()`;
- `put/get/get_with_options/delete/write` по `ColumnFamilyId`;
- `scan/snapshot/flush/compact_range/begin_transaction` в пределах одной family.

Имена family: 1–64 ASCII символов `A-Za-z0-9_-`; `default` зарезервировано за ID 0. Созданный ID сохраняется после закрытия и перезапуска. `drop_family` логически удаляет family: старый каталог не удаляется и ID не переиспользуется.

### Crash-safe CF Registry V1

Отдельный `CF_REGISTRY V1` хранится в `.forgedb-column-families/REGISTRY`: magic/version, monotonic next ID, отсортированный ID↔name mapping, bounded count/lengths и CRC32C. Publication выполняется через запись/sync временного snapshot, atomic rename и sync directory. Сначала инициализируется child DB, только затем публикуется registry.

Повреждённый registry, неизвестная версия или отсутствие зарегистрированного child каталога приводит к явной ошибке. Незарегистрированные/orphan директории не подключаются и не удаляются автоматически; symbolic links в reserved paths отклоняются.

### Команды CLI

```bash
forgedb cf create ./data users
forgedb cf list ./data
forgedb cf put ./data 1 user42 Alice --sync
forgedb cf get ./data 1 user42
forgedb cf scan ./data 1
forgedb cf delete ./data 1 user42 --sync
forgedb cf drop ./data 1
```

ID `1` в примере условный: используйте фактический ID из `cf create` или `cf list`. CLI доступен без установки Rust из assets этого release.

## Подтверждённая изоляция

Данные одноимённого user key в `default` и именованных families не пересекаются, включая delete/tombstones. Flush, leveled compaction и WAL recovery выполняются в независимых каталогах. Snapshot и optimistic transaction относятся к конкретной family. При закрытии facade синхронизируются открытые child databases и default root; повторный close идемпотентен.

## Важные ограничения

- Это первая реализация **physical-isolation** Column Families, а не несколько CF внутри единого MANIFEST/SSTable. Каждый CF имеет свой WAL, MANIFEST, sequence-number timeline и lock.
- **Нет атомарного WriteBatch или transaction между несколькими families.** Внутри выбранной family обычный `ColumnFamilies::write(id, WriteBatch)` и `begin_transaction(id)` используют прежние WAL/MVCC guarantees.
- `WriteBatch::put_cf` с ненулевым ID пока намеренно отклоняется старым storage core. Для именованных families используйте `ColumnFamilies::write(id, обычный batch)`.
- Нет глобального snapshot и serializable isolation между families. Каждая family использует свою конфигурацию памяти и кэш; общего RAM/FD лимита нет.
- Drop не выполняет destructive cleanup; orphan каталоги сохраняются. Автоматическая cross-restart GC, unified WAL, фоновая compaction, FQL/SQL не добавлены.
- `Database::open` остаётся API для **default** root, не перечисляет именованные families.

## Совместимость persisted-файлов

Для default и каждого child каталога не изменены: IDENTITY V1, Internal Key V1, WAL V1, SSTable V1, MANIFEST writer V2/reader V1–V2 и CURRENT V1. Добавлен **только новый отдельный** CF_REGISTRY V1, без миграции старых данных.

## Проверки

Feature PR [#33](https://github.com/Onmaynec/ForgeDB/pull/33), head `2f53fbaf72558093870275e1353d27b69f7c97fd`, [CI run #43552878243](https://github.com/Onmaynec/ForgeDB/actions/runs/43552878243):

- Ubuntu stable: rustfmt, strict Clippy -D warnings, workspace tests, doctests, release build, benchmark smoke — success;
- Windows stable: аналогичные проверки — success;
- Rust 1.89: strict Clippy и workspace tests — success.

Новые тесты: same-key CF isolation, reopen/recovery, independent snapshots/transactions, drop/recreate ID, read-only, corrupted/future registry, missing registered child, crash orphan retention и CLI multi-process CRUD. Release workflow отдельно повторяет Linux gate до создания annotated tag.

## Готовые CLI-бинарники

Для Windows и Linux x86-64 собираются ZIP/TAR.GZ архивы с отдельными SHA-256 файлами:

- `forgedb-v0.7.0-windows-x86_64.zip` — `forgedb.exe`;
- `forgedb-v0.7.0-linux-x86_64.tar.gz` — `forgedb`.

Инструкция по распаковке и проверке checksum: `docs/QUICKSTART.md`.

## Документация

- `docs/releases/V0.7.0.md`;
- `docs/formats/CF_REGISTRY_V1.md`;
- `docs/adr/0015-column-family-isolation.md`;
- `docs/ARCHITECTURE.md`, `docs/COMPATIBILITY.md`, `docs/VALIDATION.md`, `docs/INVARIANTS.md`.
