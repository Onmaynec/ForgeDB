# Crash recovery matrix ForgeDB v0.2.0

Этот документ задаёт проверяемый контракт создания базы, WAL rotation, SSTable build и MANIFEST installation.
Он не заменяет автоматические тесты: до зелёного Linux/Windows/MSRV gate все пункты имеют статус **draft**.

## Термины

- **current WAL** — поколение `MANIFEST.log_number`, принимающее новые записи;
- **previous WAL** — поколение `MANIFEST.previous_log_number`, закреплённое за immutable memtable;
- **reserved SST** — номер SSTable, зарезервированный rotation edit;
- **installed SST** — файл, присутствующий в `MANIFEST.l0_files`;
- **authoritative** — достижимый из `CURRENT` и успешно replay’нутого MANIFEST;
- **orphan** — файл, который не входит в authoritative state.

## Bootstrap новой базы

Порядок публикации:

1. создать или проверить `IDENTITY`;
2. создать и синхронизировать header-only `WAL-000001.log`;
3. создать и синхронизировать initial `MANIFEST-000001`;
4. атомарно установить и синхронизировать `CURRENT`;
5. только после этого вернуть открытый `Database` вызывающему коду.

| Точка падения | Состояние на диске | Поведение следующего open |
|---|---|---|
| До `IDENTITY` | каталог может отсутствовать или быть пустым | обычная новая инициализация |
| После `IDENTITY`, до WAL | `IDENTITY`, без `CURRENT` | создать initial WAL, затем MANIFEST/CURRENT |
| После fsync WAL, до MANIFEST | `IDENTITY`, header-only WAL, без `CURRENT` | строго проверить и повторно использовать только пустой WAL |
| После MANIFEST rename, до `CURRENT` | WAL и orphan `MANIFEST-000001`, без `CURRENT` | удалить только reserved bootstrap orphan, построить MANIFEST заново |
| После записи `CURRENT.tmp`, до rename | WAL, MANIFEST, `CURRENT.tmp`, без `CURRENT` | удалить только `CURRENT.tmp` и reserved MANIFEST artifacts, повторить bootstrap |
| После rename `CURRENT`, до directory fsync | `CURRENT` может быть видим | если `CURRENT` видим — normal recovery; иначе bootstrap retry |
| После успешного `CURRENT` | authoritative WAL и MANIFEST | отсутствие указанного WAL является corruption, а не поводом создать новый файл |

Пользовательские и неизвестные файлы bootstrap cleanup не удаляет.

## Обычная запись

Для `Durability::Sync`:

1. batch полностью кодируется;
2. WAL frame полностью записывается;
3. WAL синхронизируется;
4. подготовленный batch атомарно публикуется в mutable memtable.

Падение до пункта 3 не обещает durable visibility. Падение после пункта 3 обязано восстановить batch целиком.

## Начало flush и WAL rotation

Порядок:

1. синхронизировать current WAL;
2. зарезервировать `table_file_number = next_file_number`;
3. вычислить новое WAL-поколение;
4. создать или строго проверить header-only файл нового WAL;
5. append + fsync rotation edit в MANIFEST;
6. только после успешного edit заменить mutable на immutable и переключить writer на новый WAL.

| Точка падения | Authoritative state | Recovery |
|---|---|---|
| До sync current WAL | старый current WAL | normal WAL replay |
| После sync, до создания нового WAL | старый current WAL | повторить rotation |
| После создания нового WAL, до MANIFEST edit | старый current WAL; новый WAL — orphan | повторно использовать только если файл header-only, UUID и file number совпадают |
| Новый orphan WAL содержит frame или partial tail | старый current WAL | flush завершается corruption; файл не удаляется и не перезаписывается |
| Во время append rotation edit | зависит от полного checksummed frame | полный edit replay’нется; неполный последний frame будет отброшен по MANIFEST tail policy |
| После fsync rotation edit, до in-memory switch | MANIFEST содержит current+previous WAL | recovery разделяет previous и current в разные memtable |

## Recovery двух WAL-поколений

При `previous_log_number = Some(n)`:

- previous WAL replay’ится только в immutable pending memtable;
- current WAL replay’ится только в mutable memtable;
- point read идёт `mutable → immutable → L0 newest-first`;
- новый rotation запрещён, пока pending immutable не установлен;
- reserved SST восстанавливается детерминированно из rotation generation;
- отсутствие current или previous WAL — corruption.

Смешивание двух WAL в одну mutable memtable запрещено: оно делает возможной потерю previous generation после второго crash.

## SSTable build

Порядок:

1. удалить только temp/final SST с тем же доказанным reserved number, если он не live;
2. записать SST во временный каталог;
3. синхронизировать SST;
4. атомарно переместить его в каталог базы;
5. синхронизировать destination directory;
6. открыть SST reader и полностью проверить header/footer/checksums/index/filter/properties;
7. только затем переходить к MANIFEST installation.

| Точка падения | Состояние | Recovery |
|---|---|---|
| Во время temp write | partial temp SST | удалить только temp с reserved number и построить заново |
| После fsync temp, до rename | complete temp SST | deterministic rebuild; temp не считается live |
| После rename, до MANIFEST edit | complete orphan final SST | удалить только доказанный reserved orphan и построить заново |
| Orphan SST имеет другой номер | неизвестный/non-live файл | normal open не включает его в VersionSet и не удаляет автоматически |
| SST checksum/metadata неверны | malformed orphan или live file | до installation — rebuild reserved orphan; после installation — open возвращает corruption |

## MANIFEST installation

Installation edit содержит:

- `added_files = [new SST metadata]`;
- `previous_log_number = None`;
- monotonic `last_sequence`.

Порядок:

1. preflight применить edit к clone состояния;
2. записать полный checksummed MANIFEST frame;
3. fsync MANIFEST;
4. опубликовать новый in-memory state;
5. добавить проверенный reader;
6. очистить pending immutable;
7. только после этого удалить obsolete previous WAL;
8. синхронизировать каталог после удаления.

| Точка падения | Recovery |
|---|---|
| До MANIFEST append | previous WAL остаётся authoritative; SST является orphan |
| На неполном последнем MANIFEST frame | tail отбрасывается; previous WAL остаётся authoritative |
| После полного append, до fsync | durability зависит от ОС; recovery принимает только реально сохранившийся полный frame |
| После fsync, до in-memory publish | MANIFEST authoritative; open читает установленный SST |
| После publish, до удаления old WAL | SST authoritative, старый WAL obsolete; лишний WAL безопасен |
| После удаления old WAL, до directory fsync | MANIFEST уже не ссылается на WAL; reopen читает SST |

Старый WAL никогда не удаляется до успешного installation edit.

## Empty previous WAL

Header-only previous WAL может появиться после crash вокруг rotation. Flush не создаёт пустой SSTable:

1. append installation edit только с `previous_log_number = None`;
2. fsync MANIFEST;
3. удалить obsolete empty WAL;
4. сохранить current mutable generation без изменений.

## Concurrency

- одновременно строится не более одного flush;
- второй `flush()` получает `Busy`;
- `close()` во время build получает `Busy` и не публикует `closed`;
- записи после rotation продолжаются в current WAL/mutable;
- installation проверяет, что pending generation не изменилась.

## Обязательные автоматические проверки перед релизом

- bootstrap WAL-before-CURRENT;
- retry после header-only initial WAL;
- retry после orphan initial MANIFEST/CURRENT.tmp;
- отказ повторной инициализации при существующем CURRENT;
- retry после header-only orphan next WAL;
- отказ и сохранение non-empty orphan next WAL;
- previous/current WAL recovery в разные memtable;
- reopen после SST installation без obsolete WAL;
- checksum corruption live SST;
- orphan SST не влияет на reads;
- missing MANIFEST-owned WAL блокирует open;
- model test через несколько flush/reopen;
- Ubuntu stable, Windows stable, Rust 1.89;
- `cargo fmt`, `clippy -D warnings`, unit/integration/doc tests и release build с `--locked`.
