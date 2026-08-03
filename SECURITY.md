# Security Policy

ForgeDB is pre-1.0 software and is not ready for production data.

## Reporting a vulnerability

Please use GitHub private vulnerability reporting for the repository. Do not publish exploit details
in a public issue before maintainers have had an opportunity to investigate.

## Current security scope

The v0.0.1 foundation treats paths, configuration, identity files, internal-key bytes, and batch
contents as untrusted. Parsers use checked arithmetic and configured allocation limits. The project
does not yet provide encryption at rest, authentication, secure deletion, or tamper-proof storage.
