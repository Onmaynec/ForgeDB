//! Публичный API ForgeDB v0.4.0.
//!
//! Development milestone добавляет leveled compaction contracts, MANIFEST V2,
//! bounded merge/output splitting и crash-safe migration поверх full LSM read path.

mod database_v3_fixed {
    include!("database_v3_fixed.rs");
    include!("database_compaction.rs");
    #[cfg(test)]
    include!("database_compaction_crash_tests.rs");
    include!("database_obsolete.rs");
    include!("database_stall.rs");
    include!("database_streaming_wrapper.rs");
    include!("database_compaction_wrapper.rs");
}
mod entry_cursor;
mod inventory;
mod iterator;
mod manifest_runtime;
mod mvcc_memtable;
mod options;
mod read_options;
mod recovery_plan;
mod snapshot;
mod sst_entry_cursor;
mod table_scan;
mod verify;
mod version_set {
    include!("version_set.rs");
    include!("version_set_compaction.rs");
}
mod write_options;

pub use database_v3_fixed::StreamingDatabase as Database;
pub use forgedb_core::{
    BatchOperation, ColumnFamilyId, DatabaseId, Error, InternalKey, InternalKeyCodec,
    InternalKeyComparator, Limits, Result, SequenceNumber, ValueType, WriteBatch,
};
pub use inventory::FileInventory;
pub use iterator::DbIterator;
pub use options::Options;
pub use read_options::ReadOptions;
pub use recovery_plan::RecoveryPlan;
pub use snapshot::Snapshot;
pub use verify::{VerifyOptions, VerifyReport};
pub use write_options::{Durability, WriteOptions};
