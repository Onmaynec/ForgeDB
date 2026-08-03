# ADR 0002: Start with one logical writer

- Status: Accepted
- Date: 2026-08-03

The first durable write path will serialize commits through one logical writer. Concurrent readers
remain a goal. Group commit may be added only after the baseline order, error propagation, atomicity,
and visibility rules are proven.
