# LSM read path

Полный LSM read path реализован в ForgeDB v0.3.0 и используется в v0.4.0. Этот документ оставлен как точка входа для ссылок на первоначальный milestone.

В текущей реализации чтение объединяет mutable/immutable memtables и L0/L1+ SSTables с учётом snapshots и tombstones. Публичный range scan использует forward merge iterator и block-at-a-time SSTable cursors; старый materialized scan остаётся внутренним переходным кодом.

Актуальные контракты и ограничения описаны в [READ_PATH.md](READ_PATH.md), [SNAPSHOTS_AND_MVCC.md](SNAPSHOTS_AND_MVCC.md) и [ARCHITECTURE.md](ARCHITECTURE.md).
