# Совместимость ForgeDB

## Версия библиотеки

Текущий development milestone: `v0.4.0`. Публичный API может изменяться до `v1.0.0`, но persisted-форматы меняются только через явную новую version, fixtures, rejection tests и документированную migration policy.

## Поддерживаемый Rust

Минимальная поддерживаемая версия: Rust 1.89. CI также проверяет текущий stable на Ubuntu и Windows.

## Persisted-форматы

| Компонент | Active writer | Reader | Статус |
|---|---:|---:|---|
| `IDENTITY V1` | 1 | 1 | стабилен |
| `Internal Key V1` | 1 | 1 | стабилен |
| `WAL V1` | 1 | 1 | стабилен |
| `SSTable V1` | 1 | 1 | стабилен |
| `CURRENT V1` | 1 | 1 | стабилен |
| `MANIFEST V1` | нет | 1 | legacy read-compatible |
| `MANIFEST V2` | 2 | 2 | active development writer, level-aware |

Unknown future version каждого формата отклоняется. Reader не пытается интерпретировать future version как текущий layout.

## MANIFEST compatibility

### V1

MANIFEST V1 хранит только L0 metadata. Его binary layout и semantics не изменяются. Fixed fixture находится в:

```text
crates/forgedb-manifest/fixtures/manifest-v1-minimal.hex
```

Compatibility test replayed fixture существующим V1 decoder, поэтому доказательство не зависит от текущего encoder.

Read-only open V1 создаёт только in-memory level-aware bridge и не изменяет каталог.

Read-write open V1 выполняет migration под exclusive database lock:

1. replay и validate V1;
2. выбрать новый unified file number, пропуская existing MANIFEST/temp/WAL/SST names;
3. записать полный V2 snapshot во временный MANIFEST;
4. sync file;
5. rename и sync directory;
6. опубликовать новое имя через `CURRENT.tmp`;
7. sync и atomic rename CURRENT;
8. открыть V2 writer.

Старый V1 MANIFEST остаётся obsolete и не перезаписывается.

### V2

MANIFEST V2 добавляет:

- exact level в add/delete file records;
- authoritative level count;
- compaction pointers;
- L1+ sorted/non-overlap validation;
- minimum reader version 2.

V2 имеет отдельные header/frame/edit versions и документирован в [`formats/MANIFEST_V2.md`](formats/MANIFEST_V2.md).

V2 writer используется production Database path milestone v0.4. Physical compaction installation записывает только V2 records и не кодирует level metadata в V1 reserved bytes.

## Migration crash outcomes

До публикации нового CURRENT authoritative остаётся V1. После sync/rename CURRENT authoritative становится V2. Подозрительный existing temp/final MANIFEST не перезаписывается.

Orphan V2 MANIFEST, не выбранный CURRENT, не открывается автоматически.

## Compaction compatibility

Compaction не меняет WAL V1, SSTable V1 или Internal Key V1. Outputs используют обычный SSTable V1 writer. Только level membership и add/delete metadata записываются в MANIFEST V2.

Process crash до final compaction edit оставляет old Version live и output SST orphan. Process crash после final MANIFEST sync восстанавливает new Version; input SST могут остаться безопасными orphan files.

## Совместимость WAL

WAL V1 привязан к database identity, canonical file number, little-endian layout, CRC32C Castagnoli и operation payload V1. Изменение обязательного поля требует WAL V2 или отдельного совместимого extension protocol.

## Операционные системы

Поддерживаемый CI:

- Ubuntu latest;
- Windows latest;
- Ubuntu с MSRV Rust 1.89.

Native lock, directory sync и truncation имеют platform-specific реализации. Другие платформы не считаются поддерживаемыми без отдельного CI и crash-тестов.

Process-crash compaction matrix проходит Linux и Windows. Это не означает одинаковые OS-crash или power-loss guarantees.

## Endianness и размер указателя

Persisted integers имеют фиксированную ширину и little-endian порядок. Значения `usize` не записываются напрямую. Все преобразования между persisted `u32/u64` и `usize` проверяются до allocation, indexing или filesystem read.

## Обратная совместимость API

До v1.0 SemVer допускает несовместимые изменения minor-версии. Release notes обязаны перечислять:

- изменения публичного API;
- изменения persisted-форматов;
- migration requirements;
- изменения durability semantics.
