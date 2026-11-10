# Совместимость ForgeDB

## Версия библиотеки

`v0.5.0` является pre-release milestone. Публичный API может изменяться до `v1.0.0`, но persisted-форматы меняются только через явную новую версию, ADR, compatibility tests и документированную migration policy.

## Поддерживаемый Rust

Минимальная поддерживаемая версия: Rust 1.89. CI также проверяет текущий stable на Ubuntu и Windows.

## Persisted-форматы v0.5.0

| Компонент | Writer | Reader | Неизвестная будущая версия |
|---|---|---|---|
| `IDENTITY V1` | V1 | V1 | Отклоняется |
| `Internal Key V1` | V1 | V1 | Отклоняется |
| `WAL V1` | V1 | V1 | Отклоняется |
| `SSTable V1` | V1 | V1 | Отклоняется |
| `MANIFEST` | V2 | V1, V2 | Отклоняется |
| `CURRENT V1` | V1 | V1 | Отклоняется |

Reader не пытается интерпретировать future version как текущий layout.

## Transaction compatibility v0.5.0

v0.5.0 не вводит отдельный persisted transaction format.

Successful transaction commit после conflict validation передаёт полный write set существующему `Database::write` одним `WriteBatch`. WAL V1 сохраняет этот batch одним logical frame, поэтому transaction atomicity использует уже существующий WAL contract.

Следовательно:

- WAL V1 layout не меняется;
- SSTable V1 layout не меняется;
- MANIFEST V2 layout не меняется;
- база v0.4.0 не получает новый неизвестный file type только из-за transactional writes;
- isolation semantics являются library/API contract v0.5.0, а не новым persisted encoding.

`DisableWal` для transaction commit запрещён и не создаёт отдельный compatibility mode.

## MANIFEST V1 → V2

MANIFEST V2 введён в v0.4.0 для persisted L1+ placement.

- read-only open V1 базы не изменяет каталог;
- read-write open V1 сначала полностью replay/validate старый MANIFEST;
- затем создаётся новый V2 snapshot под новым unique file number;
- V2 sync/rename выполняются до replacement `CURRENT`;
- V1 MANIFEST не перезаписывается in-place;
- после durable publication V2 старая ForgeDB v0.3.x должна отказать с future-format error, а не угадывать layout.

Подробный контракт: `docs/formats/MANIFEST_V2.md` и ADR 0010.

## WAL V1

WAL V1 привязан к database identity, canonical numbered generation, little-endian layout, CRC32C Castagnoli и operation payload V1. Изменение обязательного layout требует WAL V2 или отдельного совместимого extension protocol.

Transaction commit v0.5.0 не добавляет новый WAL record type: используется тот же atomic WriteBatch frame.

## SSTable V1

Compaction и transaction support v0.5.0 не меняют SSTable layout. Input и output tables используют один и тот же SSTable V1 contract; level placement является MANIFEST metadata, а transaction conflict validation читает обычные MVCC sequence numbers из существующих internal keys.

## Операционные системы

Поддерживаемый CI:

- Ubuntu latest;
- Windows latest;
- Ubuntu с Rust 1.89 как MSRV gate.

Native lock, directory sync, atomic rename и truncation имеют platform-specific implementations. Другие платформы не считаются поддерживаемыми без отдельного CI/crash coverage.

## Endianness и размер указателя

Persisted integers имеют фиксированную ширину и little-endian порядок. `usize` не записывается напрямую. Все преобразования persisted `u32/u64` ↔ `usize` проверяются.

## Обратная совместимость API

До v1.0 SemVer допускает несовместимые изменения minor-версии. Release notes обязаны перечислять изменения public API, persisted formats, migration requirements и durability semantics.
