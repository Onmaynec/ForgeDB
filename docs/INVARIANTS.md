# ForgeDB invariants

Released guarantees require green automated evidence. v0.2/v0.3 invariants are implemented in draft but unverified: crash-safe SST installation, L0 newest-first, L1+ non-overlap, snapshot visibility, bounded cursor reads, nondestructive recovery planning and unchanged public/persisted contract through the temporary streaming wrapper.

Compaction lifetime/tombstone invariants are planned for v0.4. Salvage immutability is planned for v0.9.
