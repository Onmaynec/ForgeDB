# Validation

Every milestone commit must pass the repository's Linux, Windows, and minimum-supported-Rust jobs.
The canonical local command sequence is maintained in `scripts/check.sh` and `scripts/check.ps1`.

A release is created only after formatting, Clippy, unit and property tests, doctests, and a release
build complete successfully. Windows and minimum-supported-Rust results are treated as release
requirements, not optional signals.
