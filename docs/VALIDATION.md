# Контракт проверки ForgeDB v0.4.0

Каждое изменение core, persisted-форматов или recovery обязано проходить GitHub Actions на Linux, Windows и минимально поддерживаемой версии Rust 1.89. Успешный compile не заменяет unit, model, integration и crash tests.

## Локальная проверка

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked --workspace --doc
cargo build --locked --workspace --release
```

MSRV jobs:

```bash
cargo +1.89 clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo +1.89 test --locked --workspace --all-features
```

Для локальной разработки есть `scripts/check.sh` и `scripts/check.ps1`.

## Обязательные направления тестирования

- WAL atomic batch, checksum, partial tail, sequence gaps и replay;
- FLUSH: WAL rotation, previous/current recovery, retry после crash;
- SSTable: bounded decoding, checksums, block boundaries, Bloom filter;
- MANIFEST V1/V2 compatibility, migration, future-format rejection;
- snapshot visibility и MVCC retention;
- L0→L1 и Ln→Ln+1 compaction, user-key-safe output splitting;
- tombstone dropping только при bottommost/snapshot-safe условиях;
- duplicate/overlap/absent file detection;
- process-abort boundaries, reopen и file inventory/verify;
- Windows lock/sync/rename semantics и Rust 1.89.

Новые persisted-форматы требуют ADR, фиксированного layout, fixtures, migration/rollback и rejection tests.

## Release gate

Тег и GitHub Pre-release публикуются только после успешных канонических проверок, подтверждения версии workspace, обновления русских release notes и самостоятельной проверки recovery/durability impact. Нельзя обходить Clippy или отключать платформенный тест ради зелёного CI.

Проверенные run ID указываются в описании PR; статус тестов не переносится с другого commit. Политика выпусков: [RELEASE_POLICY.md](RELEASE_POLICY.md).
