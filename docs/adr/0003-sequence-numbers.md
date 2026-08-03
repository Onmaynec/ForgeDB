# ADR 0003: глобальные ненулевые sequence numbers

- Статус: принято
- Дата: 2026-08-03

## Решение

Каждая mutation получает глобальный `u64` sequence number. Ноль зарезервирован. Арифметика всегда checked;
исчерпание диапазона является fatal resource-limit error и никогда не приводит к wraparound.

Операции одного `WriteBatch` получают непрерывный диапазон. Internal keys сравнивают sequence по убыванию,
чтобы более новая версия одного user key располагалась раньше старой.

## DisableWal

`DisableWal` всё равно резервирует sequence range для runtime ordering. Поэтому WAL может содержать возрастающие
пропуски. Replay разрешает gaps, но запрещает повтор, пересечение и уменьшение persisted sequence range.

## Последствия

Sequence number связывает WAL order, memtable versions, будущие snapshots и SSTable entries. Его semantics нельзя
изменять без анализа совместимости всех persisted-компонентов.
