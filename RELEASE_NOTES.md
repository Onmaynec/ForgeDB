# ForgeDB v0.2.0 — SSTable and Flush

Статус выпуска: GitHub Pre-release.

ForgeDB получила первый persisted LSM-слой: данные переходят из WAL и mutable memtable в проверенные immutable SSTable, а authoritative состояние каталога восстанавливается через MANIFEST и CURRENT.

## Основные изменения

- публичный `Database::flush()`;
- mutable и immutable memtable generations;
- SSTable V1 с фиксированными header/footer;
- checksummed data, index, Bloom filter и properties blocks;
- prefix compression и restart points;
- MANIFEST V1 с checksummed `VersionEdit` frames;
- crash-safe `CURRENT`;
- numbered WAL generations;
- point reads `mutable → immutable → L0 newest-first`;
- типизированный `TableBuildRequest`;
- зафиксированный dependency graph через `Cargo.lock` и `--locked` release gate.

## Crash consistency

### WAL-first bootstrap

При создании базы initial WAL создаётся и синхронизируется до MANIFEST и `CURRENT`. После публикации `CURRENT` отсутствие указанного current или previous WAL считается corruption и не маскируется созданием пустого replacement-файла.

### Generation-safe recovery

Если процесс завершился после WAL rotation, но до SST installation:

- previous WAL восстанавливается в immutable pending memtable;
- current WAL восстанавливается в mutable memtable;
- reserved SST generation сохраняется;
- следующий rotation запрещён до установки pending generation.

Это закрывает double-crash сценарий, в котором подтверждённые записи могли быть исключены из authoritative state.

### Retry-safe orphan handling

- header-only orphan WAL повторно используется только после проверки database UUID, file number и отсутствия complete frame или partial tail;
- non-empty/partial orphan WAL не удаляется и блокирует flush;
- повторный SST build удаляет только доказанный non-live reserved SST number;
- unknown и пользовательские файлы normal recovery не удаляет.

### Durable installation

SSTable становится live только после полной проверки и durable installation edit в MANIFEST. Obsolete previous WAL удаляется только после успешного sync этого edit.

## Corruption policy

- все persisted headers, offsets, lengths, reserved fields и CRC32C проверяются;
- неизвестные будущие версии форматов отклоняются;
- checksum mismatch live SSTable блокирует open;
- corruption-тесты детерминированно изменяют сохранённый CRC через XOR;
- normal recovery не выполняет resynchronization после mid-file corruption.

## Persisted-форматы

| Формат | Reader | Writer |
|---|---:|---:|
| IDENTITY | 1 | 1 |
| Internal Key | 1 | 1 |
| WAL | 1 | 1 |
| SSTable | 1 | 1 |
| MANIFEST | 1 | 1 |
| CURRENT | 1 | 1 |

`IDENTITY V1`, `Internal Key V1` и `WAL V1` сохраняются. Добавлены `SSTable V1`, `MANIFEST V1` и `CURRENT V1`.

## Проверка

Feature candidate прошёл GitHub Actions run `31004356415`:

- Ubuntu stable: rustfmt, strict Clippy с `-D warnings`, unit/integration tests, doctests, release build;
- Windows stable: тот же полный набор;
- Ubuntu Rust 1.89: strict Clippy и unit/integration tests.

Release PR повторно проходит ту же cross-platform matrix. После merge release workflow ещё раз выполняет полный Linux gate до создания annotated tag и GitHub Pre-release.

## Ограничения

Версия намеренно ограничена Level 0 и пока не содержит:

- leveled compaction;
- range iterator;
- snapshots;
- transactions;
- background flush worker и write stalls;
- CLI inventory/verify.

SSTable reader v0.2 bounded-загружает файл целиком. Эти ограничения устраняются в следующих milestone.
