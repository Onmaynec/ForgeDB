# ForgeDB v0.1.0 — Durable WAL Core

Статус выпуска: GitHub Pre-release.

ForgeDB получила первый рабочий storage path: атомарные batch записываются в versioned WAL, становятся видимыми
в bounded mutable memtable и восстанавливаются после повторного открытия.

## Основные изменения

- публичный `Database` API: `open`, `put`, `get`, `delete`, `write`, `close`;
- бинарные ключи и значения;
- атомарный `WriteBatch`;
- WAL V1 с CRC32C Castagnoli;
- строгий bounded decoder и отказ от resync после corruption;
- mutable memtable с preflight memory accounting;
- `Sync`, `NoSync` и `DisableWal`;
- replay полных кадров и безопасное truncation последнего partial frame;
- sequence numbers с запретом пересечений и допустимыми gaps после `DisableWal`;
- read-only open с полным replay;
- CLI `put/get/delete`;
- process-abort, model, corruption и cross-platform tests.

## Контракт Sync

Успешный `Sync` означает следующий порядок:

```text
WAL write_all
→ WAL sync_all
→ атомарное применение batch к memtable
→ возврат успеха
```

Process-abort test подтверждает восстановление такой записи после немедленного аварийного завершения процесса.
Power-loss гарантия всё ещё зависит от ОС, файловой системы, накопителя и контроллера.

## Recovery policy

- Заголовок WAL и каждый полный кадр проверяются по структуре, версии, identity, лимитам и CRC32C.
- Batch полностью декодируется до применения.
- Только последний физически неполный кадр может быть отброшен.
- При read-write открытии partial tail усекается до последней проверенной границы и синхронизируется.
- Mid-file corruption и checksum mismatch полного кадра блокируют открытие.

## Совместимость

Добавлен persisted-формат `WAL V1`. `IDENTITY V1` и `Internal Key V1` сохранены без несовместимых изменений.
Неизвестные будущие версии отклоняются.

## Проверка

Финальный release candidate проверен GitHub Actions run `30802178049`:

- Ubuntu stable: rustfmt, Clippy с `-D warnings`, workspace tests, doctests, release build;
- Windows stable: те же проверки;
- Ubuntu Rust 1.89: Clippy с `-D warnings` и workspace tests;
- отдельный release job повторяет полный Linux gate до создания tag и Pre-release.

## Ограничения

Версия намеренно не содержит:

- SSTable;
- MANIFEST;
- memtable flush;
- compaction;
- snapshots и transactions;
- iterators и range scans;
- WAL rotation;
- background workers.

Все пользовательские данные v0.1.0 восстанавливаются из одного `WAL-000001.log` в mutable memtable при каждом открытии.
При достижении `Options::memtable_size` новая запись отклоняется до изменения WAL.
