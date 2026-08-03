# Контракт проверки

Каждый milestone commit обязан проходить Linux, Windows и minimum-supported-Rust jobs.

## Канонические команды

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
```

Для MSRV:

```bash
cargo +1.89 clippy --workspace --all-targets --all-features -- -D warnings
cargo +1.89 test --workspace --all-features
```

Локальные последовательности также находятся в `scripts/check.sh` и `scripts/check.ps1`.

## Обязательные классы тестов v0.1.0

- unit tests кодеков, limits, batch и memtable;
- malformed input и future-version rejection;
- trailing partial WAL frame;
- checksum mismatch полного кадра;
- atomic batch replay;
- deterministic model put/delete/reopen;
- process abort после успешной `Sync`-записи;
- `DisableWal` sequence gap перед durable записью;
- повторное открытие после `Database::close`;
- Windows lock, directory sync и truncation.

## Правило выпуска

Tag и GitHub Pre-release создаются только после успешных:

- rustfmt;
- Clippy без warnings;
- unit/integration/model/crash tests;
- doctests;
- release build;
- Windows и Rust 1.89 jobs.

Пропуск platform-specific failure, отключение теста или ослабление lint не считается исправлением.
