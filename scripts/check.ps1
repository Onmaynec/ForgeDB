$ErrorActionPreference = "Stop"

function Invoke-Cargo {
    & cargo @args
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($args -join ' ') failed with code $LASTEXITCODE"
    }
}

Invoke-Cargo fmt --all --check
Invoke-Cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
Invoke-Cargo test --locked --workspace --all-features
Invoke-Cargo test --locked --workspace --doc
Invoke-Cargo doc --locked --workspace --no-deps
Invoke-Cargo build --locked --workspace --release
