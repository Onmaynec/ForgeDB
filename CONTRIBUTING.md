# Участие в разработке ForgeDB

ForgeDB ставит корректность, восстановимость и понятные контракты выше результатов benchmark.

## Перед отправкой изменений

1. Опишите инвариант или контракт, которого касается изменение.
2. Добавьте unit и integration tests, включая malformed input и граничные случаи.
3. Зафиксируйте влияние на crash consistency и persisted-форматы.
4. Обновите русскую документацию и release notes.
5. Выполните форматирование, Clippy, тесты, doctests и release build.
6. Не подключайте готовый storage backend: SQLite, RocksDB, LevelDB, sled, redb и аналоги.
7. Не добавляйте `unsafe`, `todo!`, `unimplemented!` и runtime-вызовы `unwrap`/`expect`.
8. Не скрывайте platform-specific падения через `ignore` или отключение Windows CI.

## Обязательные команды

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

Изменения core-кода также должны проходить на минимальной поддерживаемой версии Rust 1.89.

## Persisted-форматы

Изменение persisted layout требует:

- отдельного ADR;
- новой версии формата;
- точной таблицы полей и endianness;
- compatibility fixtures;
- rejection tests для неизвестной future version;
- описания migration и rollback;
- обновления durability/recovery документации.

Нельзя переиспользовать reserved fields существующей версии так, чтобы старый reader молча интерпретировал новый файл.

## Путь записи

Любое изменение write path должно отвечать на вопросы:

- когда batch получает sequence range;
- когда запись становится видимой;
- какая операция является durability boundary;
- что происходит при partial write или sync error;
- может ли recovery увидеть часть batch;
- какие ресурсы проверяются до I/O и allocation.

## Pull request

PR должен быть небольшим и проверяемым. В описании укажите:

- что изменено;
- затронутые инварианты;
- выполненные команды;
- новые тесты;
- persisted-format impact;
- crash-consistency impact;
- известные ограничения.

Вся пользовательская и нормативная документация проекта ведётся на русском языке.
