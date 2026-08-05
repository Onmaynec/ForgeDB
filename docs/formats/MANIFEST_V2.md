# MANIFEST V2

Статус: persisted contract подготовлен в milestone `v0.4.0`; active database writer переключается на V2 только после отдельного crash-safe installation slice.

## Назначение

MANIFEST V2 добавляет authoritative level metadata, необходимую для physical L1+ files и leveled compaction. V1 остаётся читаемым и не меняет semantics.

## File header

Размер: 64 bytes.

```text
offset  size  field
0       8     magic = "FORGEMAN"
8       2     format_version = 2
10      2     header_length = 64
12      4     flags = 0
16      16    database_uuid
32      8     manifest_number
40      8     created_unix_ms
48      4     minimum_reader_version = 2
52      8     reserved = 0
60      4     CRC32C bytes [0..60)
```

Unknown future version отклоняется как `UnsupportedFormat` до интерпретации payload.

## Frame header

Размер: 32 bytes.

```text
offset  size  field
0       4     magic = "FMV2"
4       4     total_frame_length
8       4     CRC32C bytes [12..end)
12      1     record_type = VersionEdit
13      1     flags = 0
14      2     manifest_format_version = 2
16      4     field_count
20      4     payload_length
24      8     reserved = 0
```

Разрешается игнорировать только physically incomplete trailing frame при filesystem replay. Complete frame с checksum mismatch является corruption.

## VersionEdit payload

Payload начинается с:

```text
edit_version: u8 = 2
reserved: [u8; 3] = 0
```

Далее идут bounded fields:

```text
tag: u8
reserved: [u8; 3] = 0
length: u32 little-endian
value: [u8; length]
```

Tags:

| Tag | Поле |
|---:|---|
| 1 | comparator |
| 2 | current log number |
| 3 | previous log number; zero означает None |
| 4 | next file number |
| 5 | last sequence; zero означает None |
| 6 | add level file |
| 7 | delete level file |
| 8 | minimum reader version |
| 9 | feature flags |
| 10 | level count |
| 11 | compaction pointer |

Unknown tag отклоняется как unsupported format.

## Add level file

```text
offset  size  field
0       4     level
4       8     file_number
12      8     file_size
20      4     smallest_user_key_length
24      4     largest_user_key_length
28      8     smallest_sequence
36      8     largest_sequence
44      8     entry_count
52      N     smallest_user_key
52+N    M     largest_user_key
```

Level ограничен `0..level_count`. Key lengths проверяются до allocation. SST metadata проходит обычную `TableMeta::validate`.

## Delete level file

```text
offset  size  field
0       4     level
4       4     reserved = 0
8       8     file_number
```

Delete обязан указывать exact source level. Удаление отсутствующего файла является corruption.

## Compaction pointer

```text
offset  size  field
0       4     source level
4       4     user_key_length
8       N     user_key
```

Pointer key проходит configured key limits.

## State invariants

- минимум два и максимум 64 levels;
- file number уникален между всеми levels;
- L0 упорядочен по возрастающему file number;
- L1+ упорядочены по smallest user key и не перекрываются;
- `next_file_number` больше всех live file numbers, WAL и MANIFEST number;
- current и previous WAL различаются;
- level count не меняется внутри одного MANIFEST;
- compaction pointer существует не более чем один на level;
- minimum reader version для V2 равна 2;
- feature flags без известной semantics отклоняются.

## Compatibility

- V1 reader/writer semantics не меняются;
- fixed V1 fixture replayed существующим V1 decoder;
- V2 имеет отдельные magic/version checks и bounded codec;
- V1 → V2 migration будет выполняться через новый MANIFEST snapshot и crash-safe CURRENT replacement;
- production writer не пишет V2 до реализации migration/install protocol и crash tests;
- V2 reader не должен интерпретировать future version как V2.
