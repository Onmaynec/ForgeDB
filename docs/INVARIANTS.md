# Инварианты ForgeDB

Released guarantees are marked **обеспечен**; v0.2/v0.3 code without green CI is **реализован в draft**.

Core released invariants: exclusive lock, nonzero monotonic sequence, atomic WAL batch replay, full batch visibility, final-partial-frame policy, pre-WAL memory check, Sync-before-visibility and close releasing resources.

Draft invariants: monotonic file numbers, crash-safe SST installation, immutable SST, L0 newest-first, L1+ non-overlap, snapshot visibility, bounded persisted decoding, fatal storage writer state, one snapshot per range scan, no full-SST materialization, nondestructive inventory/recovery plan and unchanged public/persisted contract through the temporary streaming wrapper.

Compaction lifetime and tombstone-drop invariants are planned for v0.4. Salvage immutability is planned for v0.9.

Every write/recovery/read-path change must add automated evidence; documentation alone cannot promote an invariant to **обеспечен**.
