# ForgeDB Release Policy

ForgeDB releases are milestone-based and use Semantic Versioning labels while the project is pre-1.0.

A release requires:

1. a clean, reviewable milestone boundary;
2. an updated `CHANGELOG.md` and `RELEASE_NOTES.md`;
3. successful formatting and Clippy checks with warnings denied;
4. successful workspace tests, doctests, and release builds on Linux and Windows;
5. successful checks on the minimum supported Rust version;
6. explicit documentation of crash-consistency and persisted-format impact;
7. an annotated Git tag matching the Cargo workspace version;
8. a GitHub Release created from the reviewed release notes.

Release automation is idempotent: an existing tag or GitHub Release is never overwritten. Persisted
format changes require a new format version, compatibility fixtures, rejection tests for unknown
future versions, and an ADR before publication.
