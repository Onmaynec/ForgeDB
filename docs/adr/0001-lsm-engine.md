# ADR 0001: собственный LSM storage engine

- Статус: принято
- Дата: 2026-08-03

## Решение

ForgeDB реализует собственные WAL, memtable, SSTable, MANIFEST, recovery и compaction layers.
Существующие базы данных можно изучать как reference, но нельзя использовать как storage backend проекта.

## Причины

Цель ForgeDB — построить и доказать корректность полного storage protocol, а не обернуть готовую БД новым API.
Использование SQLite, RocksDB, LevelDB, sled, redb или аналога скрыло бы ключевые persisted-инварианты.

## Последствия

Каждый milestone обязан включать:

- точные persisted-форматы;
- bounded parsing;
- crash и corruption tests;
- compatibility policy;
- эксплуатационную документацию;
- явные границы durability;
- отсутствие неподтверждённых production claims.

Стоимость решения — больший объём работы и более медленное появление feature. Это принято сознательно.
