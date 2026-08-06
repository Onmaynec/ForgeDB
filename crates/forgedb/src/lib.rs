//! Публичный API ForgeDB v0.4.0.
//!
//! Версия добавляет crash-recoverable leveled compaction, persisted L1+ metadata,
//! MVCC-aware retention, conservative tombstone dropping, output splitting и
//! L0 backpressure поверх WAL/SSTable V1 и MANIFEST V1/V2.

mod database_v3_fixed {
    include!("database_v3_fixed.rs");
    include!("database_streaming_wrapper.rs");
    include!("compaction.rs");
}
mod entry_cursor;
mod inventory;
mod iterator;
mod mvcc_memtable;
mod options;
mod read_options;
mod recovery_plan;
mod snapshot;
mod sst_entry_cursor;
mod table_scan;
mod verify;
mod version_set;
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
