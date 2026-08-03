# История изменений

В этом файле перечисляются заметные изменения ForgeDB.

До v1.0 публичный API может меняться между minor-версиями. Persisted-форматы всё равно изменяются только через явную новую версию, ADR, compatibility tests и документированную migration policy.

## [0.2.0] — в разработке

### Добавлено

- Crate `forgedb-sstable` с версионируемым форматом SSTable V1.
- Фиксированные header/footer, checksummed data/index/filter/properties blocks.
- Prefix compression с restart points и строгим bounded decoder.
- Versioned Bloom filter без false negatives для добавленных ключей.
- Полная проверка database UUID, file number, sequence range, offsets, lengths и CRC32C.
- Crate `forgedb-manifest` с MANIFEST V1, VersionEdit и crash-safe CURRENT.
- Единая монотонная нумерация WAL, SSTable и MANIFEST metadata.
- Mutable/immutable memtable lifecycle для синхронного flush.
- Двухфазный WAL rotation и SSTable installation protocol.
- `Database::flush()` и point reads в порядке mutable → immutable → L0 от новых к старым.
- Recovery текущего и previous WAL поколения через authoritative MANIFEST.
- Безопасное игнорирование unreferenced SSTable: orphan не включается в live state автоматически.
- Удаление obsolete WAL только после успешного sync MANIFEST installation edit.
- Integration tests reopen-after-flush, SST checksum corruption, orphan SST и deterministic model.
- Русские нормативные документы SSTable V1, MANIFEST V1 и CURRENT V1.

### Изменено

- Workspace version повышена до `0.2.0`.
- WAL получил canonical numbered generations.
- Memtable различает `Missing`, `Value` и `Tombstone`.
- Открытие базы проверяет все SSTables, на которые ссылается MANIFEST.

### Persisted-форматы

| Формат | Текущая версия | Чтение | Запись |
|---|---:|---:|---:|
| IDENTITY | 1 | 1 | 1 |
| Internal Key | 1 | 1 | 1 |
| WAL | 1 | 1 | 1 |
| SSTable | 1 | 1 | 1 |
| MANIFEST | 1 | 1 | 1 |
| CURRENT | 1 | 1 | 1 |

### Ограничения

- Только L0; leveled compaction ещё отсутствует.
- Range scan, snapshots и transactions ещё отсутствуют.
- Flush синхронный; background worker и write stalls появятся позже.
- SSTable reader версии 0.2 bounded-загружает файл в память.
- Orphan-файлы не удаляются автоматически и пока не имеют отдельного CLI inventory report.

## [0.1.0] — 2026-08-03

### Добавлено

- Новый crate `forgedb-wal` с форматом WAL V1.
- Фиксированный versioned-заголовок WAL и один атомарный кадр на `WriteBatch`.
- CRC32C Castagnoli для заголовка файла и каждого полного кадра.
- Bounded decoder с проверкой длин, operation count, varint, reserved fields и sequence range до allocation.
- Строгий replay без resynchronization после corruption.
- Безопасная обработка и синхронизированное усечение только последнего физически неполного кадра.
- Mutable memtable на `BTreeMap` с preflight memory accounting.
- `Database::open`, `put`, `get`, `delete`, `write` и `close`.
- Режимы `Sync`, `NoSync` и `DisableWal`.
- Атомарное применение batch после требуемой durability boundary.
- Read-only открытие с replay и запретом записи.
- CLI-команды `put`, `get` и `delete`.
- Process-abort test для подтверждённой `Sync`-записи.
- Deterministic model test, corruption tests и тест sequence-gap после `DisableWal`.
- Русская нормативная документация WAL, write/read path, recovery, durability и compatibility.

### Изменено

- Workspace version повышена до `0.1.0`.
- `Database::close` синхронизирует WAL и освобождает системную блокировку.
- Файловая абстракция получила синхронизированное `truncate_file`.
- Replay разрешает возрастающие sequence gaps, но запрещает пересечения и откат.

### Гарантии

- Успешный `Sync` выполняет WAL `sync_all` до видимости batch в memtable.
- Полный batch либо целиком replayed, либо не применяется.
- Checksum mismatch полного кадра блокирует открытие.
- Неизвестные будущие версии persisted-форматов отклоняются.

### Ограничения

- Нет SSTable, MANIFEST, flush, compaction, snapshots и transactions.
- Используется один фиксированный WAL без rotation.
- Данные находятся только в mutable memtable и восстанавливаются из WAL при каждом открытии.

## [0.0.1] — 2026-08-03

### Добавлено

- Cargo workspace с crates `forgedb`, `forgedb-core` и `forgedb-fs`.
- Rust Edition 2024 и минимальная поддерживаемая версия Rust 1.89.
- Единая модель ошибок и проверяемые `Options`.
- Примитивы `DatabaseId`, `SequenceNumber`, `ColumnFamilyId` и `ValueType`.
- Логический comparator internal keys и bounded codec Internal Key V1.
- `WriteBatch` с атомарным изменением, лимитами и checked size accounting.
- Файловая абстракция и native-реализация.
- Системная эксклюзивная блокировка базы.
- Версионируемый и синхронизируемый файл `IDENTITY`.
- Unit tests, deterministic property tests, Linux/Windows CI, документация и ADR.

### Не входило в версию

- WAL, memtable, SSTable, MANIFEST, read/write API и гарантии пользовательских данных отсутствовали.
