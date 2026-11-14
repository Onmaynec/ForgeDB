#!/usr/bin/env sh
set -eu
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked --workspace --doc
cargo doc --locked --workspace --no-deps
cargo build --locked --workspace --release
