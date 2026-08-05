# Совместимость ForgeDB

## Версия библиотеки

Текущий development milestone: `v0.4.0`. Публичный API может изменяться до `v1.0.0`, но persisted-форматы меняются только через явную новую версию, fixtures, rejection tests и документированную migration policy.

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
| `MANIFEST V1` | 1 | 1 | active production format |
| `MANIFEST V2` | ещё не активирован | 2 contract codec | development, level-aware |

Unknown future version каждого формата отклоняется. Reader не пытается интерпретировать future version как текущий layout.

## MANIFEST compatibility

### V1

MANIFEST V1 хранит только L0 metadata. Его binary layout и semantics не изменяются. В репозитории находится фиксированный fixture:

```text
crates/forgedb-manifest/fixtures/manifest-v1-minimal.hex
```

Compatibility test replayed этот fixture существующим V1 reader, поэтому проверка не зависит от текущего encoder.

### V2

MANIFEST V2 добавляет:

- level в add/delete file records;
- authoritative level count;
- compaction pointers;
- L1+ sorted/non-overlap validation;
- minimum reader version 2.

V2 имеет отдельные header/frame/edit versions и документирован в [`formats/MANIFEST_V2.md`](formats/MANIFEST_V2.md).

Active database writer остаётся V1, пока не реализованы:

1. snapshot migration V1 → V2;
2. crash-safe новый MANIFEST и CURRENT replacement;
3. open/recovery обоих форматов;
4. fixtures и process-crash matrix migration;
5. atomic compaction installation.

Нельзя записывать level metadata в V1 tags или reserved bytes.

## Совместимость WAL

WAL V1 привязан к database identity, canonical file number, little-endian layout, CRC32C Castagnoli и operation payload V1. Изменение обязательного поля требует WAL V2 или отдельного совместимого extension protocol.

## Операционные системы

Поддерживаемый CI:

- Ubuntu latest;
- Windows latest;
- Ubuntu с MSRV Rust 1.89.

Native lock, directory sync и truncation имеют platform-specific реализации. Другие платформы не считаются поддерживаемыми без отдельного CI и crash-тестов.

## Endianness и размер указателя

Persisted integers имеют фиксированную ширину и little-endian порядок. Значения `usize` не записываются напрямую. Все преобразования между persisted `u32/u64` и `usize` проверяются до allocation, indexing или filesystem read.

## Обратная совместимость API

До v1.0 SemVer допускает несовместимые изменения minor-версии. Release notes обязаны перечислять:

- изменения публичного API;
- изменения persisted-форматов;
- migration requirements;
- изменения durability semantics.
