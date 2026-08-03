#!/usr/bin/env sh
set -eu
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --doc
cargo build --workspace --release
