# Формат Internal Key V1

Internal Key V1 — bounded canonical encoding одного логического internal key. Порядок serialized bytes
**не является** порядком логического comparator.

Все многобайтные целые числа кодируются little-endian.

```text
смещение  размер  поле
0         4       magic = "FIKY"
4         1       version = 1
5         3       reserved = 0
8         4       column family ID
12        4       длина user key
16        N       bytes user key
16+N      8       sequence number
24+N      1       value type: 0 = deletion, 1 = value
```

Полная длина равна `25 + N`.

## Проверка reader

Reader отклоняет:

- неверный magic;
- unsupported version;
- ненулевые reserved bytes;
- пустой key;
- key выше настроенного лимита;
- sequence zero;
- неизвестный value type;
- integer overflow;
- truncation;
- trailing bytes.

Проверка длины выполняется до копирования и allocation user key.

## Логический comparator

1. `column_family` по возрастанию;
2. `user_key` лексикографически по возрастанию;
3. `sequence` по убыванию;
4. `value_type` по убыванию.

Такой порядок размещает более новую версию одного user key раньше старой. Нельзя сравнивать serialized keys обычным
bytewise comparator: little-endian поля и отдельный descending sequence не сохраняют нужный logical order.

## Совместимость

Любое несовместимое изменение layout требует Internal Key V2. Reader V1 не интерпретирует неизвестную будущую
версию как текущую.
