# ADR 0005: durability step предшествует видимости в memtable

Статус: принято.

## Контекст

Если batch станет видимым до успешного WAL append или обязательного sync, caller может наблюдать данные, которые
не соответствуют возвращённому результату и не восстанавливаются после process crash. Откат частично изменённой
memtable усложняет error path и создаёт риск нарушения атомарности.

## Решение

Путь записи выполняется в порядке:

```text
validation
→ sequence reservation
→ memtable preflight без изменения состояния
→ WAL append
→ optional sync согласно WriteOptions
→ атомарное применение prepared batch к memtable
→ возврат успеха
```

При `DisableWal` WAL steps сознательно отсутствуют, но preflight и атомарное применение сохраняются.

При ошибке append или sync prepared batch уничтожается без применения, sequence state не продвигается, а writer
переходит в fatal state до повторного открытия базы.

## Последствия

Положительные:

- `Sync`-успех означает, что WAL был синхронизирован до видимости;
- reader не видит batch, который write path вернул как ошибку;
- не требуется rollback memtable;
- runtime atomicity совпадает с recovery atomicity.

Отрицательные:

- writer mutex удерживается во время I/O и sync;
- latency одного slow sync блокирует другие операции;
- group commit и parallel preparation отсутствуют.

## Дальнейшее развитие

Group commit может быть добавлен без изменения внешнего контракта, если лидер группы:

1. назначает непересекающиеся sequence ranges;
2. полностью проверяет каждый batch;
3. записывает общую durable последовательность;
4. делает batch видимыми только после требуемой durability boundary.
