# Full LSM read path v0.3

Implemented in an unverified stacked draft: one snapshot per read, mutable/immutable/L0/L1+ source order, binary-heap merge, `VecEntryCursor` for memtables and block-at-a-time `SstEntryCursor` for SSTables.

The SST cursor retains a decoded index and one current data block. Public `Database` temporarily uses a streaming wrapper over the storage core. Compiler/CI validation and wrapper consolidation remain release gates.
