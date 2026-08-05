# Формат CURRENT V1

`CURRENT` выбирает единственный authoritative MANIFEST каталога базы.

## Содержимое

Файл содержит ровно одну ASCII-строку canonical-вида:

```text
MANIFEST-000001\n
```

Требования reader:

- файл не пуст;
- завершается одним LF;
- до завершающего LF нет других переводов строки;
- имя проходит строгий parser `MANIFEST-{number}`;
- абсолютные пути, разделители каталогов и `..` запрещены;
- указанный MANIFEST должен существовать и пройти полную проверку.

## Authoritative boundary

Появление валидного `CURRENT` завершает bootstrap новой базы. До его публикации initial state не считается authoritative; после публикации recovery обязано требовать существование всех current/previous WAL и live SSTable, указанных MANIFEST.

Пустой replacement WAL не создаётся для отсутствующего MANIFEST-owned файла: такой каталог открывается с corruption error.

## WAL-before-CURRENT

Initial WAL создаётся и синхронизируется раньше initial MANIFEST и `CURRENT`:

1. fsync header-only initial WAL;
2. fsync initial MANIFEST;
3. fsync `CURRENT.tmp`;
4. rename `CURRENT.tmp → CURRENT`;
5. sync каталога.

Такой порядок исключает состояние, в котором durable `CURRENT` ссылается на ещё не созданный WAL.

## Crash-safe замена

При создании или rotation:

1. записать новое содержимое в `CURRENT.tmp`;
2. выполнить file sync временного файла;
3. закрыть handle;
4. выполнить atomic rename в `CURRENT`;
5. синхронизировать каталог базы.

MANIFEST, не выбранный успешно установленным `CURRENT`, не является authoritative. Normal recovery не выбирает orphan MANIFEST по максимальному номеру автоматически.

## Bootstrap retry

Когда `CURRENT` отсутствует, реализация может детерминированно очистить только зарезервированные следы незавершённой initial installation:

- `MANIFEST-000001`;
- `MANIFEST-000001.tmp`;
- `CURRENT.tmp`.

Unknown и user files сохраняются. Header-only initial WAL может быть повторно использован только после полной проверки header, UUID, file number и отсутствия frame/tail.

Если `CURRENT` уже существует, повторная initial initialization запрещена и не перезаписывает authoritative metadata.
