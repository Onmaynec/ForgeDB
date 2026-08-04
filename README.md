# ForgeDB

Embedded Rust key-value engine. `v0.3.0` is an unverified stacked draft over unreleased `v0.2.0`, with MVCC, snapshots, L0 flush, immutable VersionSet, cursor range scan, inventory, recovery plan and verify.

SST range sources retain a decoded index and one current data block. GitHub Actions currently terminates before checkout, so merge, tag and release remain blocked. See [`docs`](docs). MIT.
