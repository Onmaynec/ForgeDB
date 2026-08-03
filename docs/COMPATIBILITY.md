# Совместимость ForgeDB

## Версия библиотеки

`v0.1.0` является pre-release. Публичный API может изменяться до `v1.0.0`, но persisted-форматы меняются только
через явную новую версию и документированную migration policy.

## Поддерживаемый Rust

Минимальная поддерживаемая версия: Rust 1.89. CI также проверяет текущий stable на Ubuntu и Windows.

## Persisted-форматы

| Компонент | Writer | Reader | Неизвестная будущая версия |
|---|---:|---:|---|
| `IDENTITY V1` | Да | Да | Отклоняется |
| `Internal Key V1` | Да | Да | Отклоняется |
| `WAL V1` | Да | Да | Отклоняется |

Reader не пытается интерпретировать future version как текущий layout.

## Совместимость WAL

WAL V1 привязан к:

- database identity;
- file number `1`;
- little-endian layout;
- CRC32C Castagnoli;
- текущему operation payload V1.

Изменение любого обязательного поля требует WAL V2 или отдельного совместимого extension protocol. Ненулевые
reserved-поля в V1 отклоняются, поэтому они не могут быть молча переиспользованы старым reader.

## Операционные системы

Поддерживаемый CI:

- Ubuntu latest;
- Windows latest.

Native lock, directory sync и truncation имеют platform-specific реализации. Другие платформы могут компилироваться,
но не считаются поддерживаемыми без отдельного CI и crash-тестов.

## Endianness и размер указателя

Persisted integers имеют фиксированную ширину и little-endian порядок. Значения `usize` не записываются напрямую.
Все преобразования между persisted `u32/u64` и `usize` проверяются.

## Обратная совместимость API

До v1.0 SemVer допускает несовместимые изменения minor-версии. Release notes обязаны перечислять:

- изменения публичного API;
- изменения persisted-форматов;
- migration requirements;
- изменения durability semantics.

## Обновление с v0.0.1

`v0.0.1` не имела пользовательского WAL или read/write path. База, созданная foundation-версией, содержит только
`IDENTITY` и `LOCK`; v0.1.0 использует тот же `IDENTITY V1` и создаёт новый `WAL-000001.log` при первом read-write открытии.
