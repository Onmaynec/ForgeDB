# MANIFEST V2

## Статус

MANIFEST V2 введён в ForgeDB v0.4.0 для authoritative persisted placement SSTables в L1+.

WAL и SSTable остаются V1. MANIFEST V1 продолжает поддерживаться на чтение.

## Compatibility

| Reader | MANIFEST V1 | MANIFEST V2 |
|---|---|---|
| ForgeDB v0.3.x | да | нет, future version rejection |
| ForgeDB v0.4.x | да | да |

Новая read-write база создаёт MANIFEST V2.

Read-only open существующей V1 базы не изменяет каталог.

Read-write open существующей V1 базы выполняет crash-safe rotation в V2:

1. replay и validate V1;
2. выделить новый unique `manifest_number` из `next_file_number`;
3. продвинуть `next_file_number`;
4. записать полный V2 snapshot во временный MANIFEST;
5. sync MANIFEST;
6. atomic rename;
7. sync database directory;
8. записать и sync `CURRENT.tmp`;
9. atomic rename в `CURRENT`;
10. sync database directory.

До шага 9 V1 остаётся authoritative. После durable CURRENT replacement authoritative становится новый V2 MANIFEST.

## Header

Header остаётся 64 bytes и сохраняет layout MANIFEST V1. Изменяется только `format_version`:

```text
offset  size  field
0       8     magic = "FORGEMAN"
8       2     format_version = 2
10      2     header_length = 64
12      4     flags/reserved = 0
16      16    database_uuid
32      8     manifest_number
40      8     created_unix_ms
48      4     minimum_reader_version
52      8     reserved = 0
60      4     CRC32C bytes [0..60)
```

Reader v0.4 принимает format versions `1..=2` и отклоняет future version.

## Frame

Frame header остаётся 32 bytes. V2 использует magic `FMV2`; V1 использует `FMV1`.

Frame format version и payload version обязаны совпадать с header version активного MANIFEST.

## VersionEdit fields

V1 tags 1–9 сохраняют прежнюю семантику.

V2 добавляет:

```text
10  ADD_LEVEL_FILE
11  DELETE_LEVEL_FILE
```

Unknown tag является unsupported format, а не silently ignored field.

### ADD_LEVEL_FILE

Payload:

```text
level: u32 little-endian
legacy_table_meta: bytes
```

`level` обязан находиться в `1..=63`.

`legacy_table_meta` использует тот же bounded TableMeta record, что V1 `ADD_FILE`; его внутреннее legacy level field остаётся `0` для стабильности вложенного layout. Реальный target level задаётся внешним V2 `level`.

### DELETE_LEVEL_FILE

Payload ровно 12 bytes:

```text
level: u32 little-endian
file_number: u64 little-endian
```

Оба значения должны быть ненулевыми; level — `1..=63`.

## State invariants

После каждого complete edit состояние проверяется:

- file number уникален между всеми уровнями;
- L0 отсортирован по file number;
- L1+ отсортированы по smallest user key;
- L1+ key ranges не перекрываются внутри одного уровня;
- `next_file_number` больше каждого использованного storage file number;
- comparator совпадает с `forgedb.internal-key.bytewise.v1`;
- unknown feature flags отклоняются;
- `last_sequence` не уменьшается.

## Compaction edit

Atomic compaction installation может одновременно содержать:

- V1 `DELETE_FILE` для L0 inputs;
- V2 `DELETE_LEVEL_FILE` для L1+ inputs;
- V2 `ADD_LEVEL_FILE` для target outputs;
- `LAST_SEQUENCE`.

Reader не должен наблюдать частично применённую compaction: frame сначала полностью читается, проверяется CRC/lengths, затем целиком применяется к cloned `ManifestState` и только после validation становится новым state.

## Unknown future versions

MANIFEST format > 2, frame version > 2, payload version > 2 или неизвестный required field tag не интерпретируются как V2 и возвращают `UnsupportedFormat`.
