# ADR 0002: один логический writer

- Статус: принято
- Дата: 2026-08-03

## Решение

Первый durable write path сериализует commit через один логический writer и один mutex состояния.
Concurrent readers остаются целью, но в v0.1.0 используют тот же mutex для атомарной видимости batch.

## Причины

До оптимизации необходимо доказать:

- единый порядок sequence numbers;
- propagation I/O errors;
- атомарность batch;
- порядок WAL append, sync и memtable visibility;
- корректное закрытие writer;
- recovery, совпадающий с runtime order.

## Последствия

Slow `sync_all` блокирует другие операции, а throughput ограничен одним writer. Group commit и более параллельный
read path могут быть добавлены только после сохранения тех же внешних контрактов и расширения crash tests.
