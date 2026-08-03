# ADR 0003: Global non-zero sequence numbers

- Status: Accepted
- Date: 2026-08-03

Every committed mutation will receive a globally monotonic `u64` sequence number. Zero is reserved.
Checked arithmetic is mandatory, and exhaustion is a fatal resource-limit error rather than wrapping.
Internal keys compare sequence numbers in descending order for each user key.
