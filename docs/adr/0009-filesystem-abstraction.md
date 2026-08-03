# ADR 0009: persisted I/O проходит через FileSystem

- Статус: принято
- Дата: 2026-08-03

## Решение

Storage-код зависит от trait `FileSystem`, а не от глобальных прямых вызовов файловой системы. Native-реализация
использует безопасные API стандартной библиотеки, включая системную блокировку файлов, доступную с Rust 1.89.

Trait покрывает:

- sequential и random-access open;
- create, create-new и append;
- rename и remove;
- list и file size;
- synchronized truncation;
- directory sync;
- exclusive lock.

## Причины

Абстракция отделяет persisted-протокол от platform-specific I/O, позволяет добавлять fault injection и делает
crash/recovery tests воспроизводимыми без внедрения storage backend.

## Platform-specific поведение

Directory sync, rename replacement semantics, locking и power-loss guarantees различаются между платформами.
ForgeDB документирует различия и не обобщает их в неподтверждённые гарантии.

## Последствия

Любая новая файловая операция storage protocol сначала добавляется в узкий trait с явным error context и тестами.
Fault-injection implementation остаётся следующим развитием после базовых process-crash tests v0.1.0.
