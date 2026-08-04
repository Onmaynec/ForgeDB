# ForgeDB

ForgeDB is an embedded Rust key-value storage engine. The current `v0.3.0` branch is an unverified stacked draft over unreleased `v0.2.0`.

Implemented source includes WAL/SSTable/MANIFEST V1, MVCC snapshots, L0 flush, immutable VersionSet, cursor range scan, inventory, recovery plan and verify. SST range sources retain a decoded index and one current data block.

GitHub Actions run `30878893688` ended before checkout with `steps=null` on Ubuntu stable, Windows stable and Rust 1.89. Local Rust tooling is unavailable. Merge, tag and release remain blocked.

Documentation: [`docs`](docs). License: MIT.
