//! Публичный API ForgeDB v0.5.0.
//!
//! Версия добавляет optimistic snapshot-isolation transactions, own-write visibility,
//! write-write conflict detection и atomic transaction commit поверх существующего single-writer
//! WAL/LSM path. Persisted WAL/SSTable/MANIFEST форматы не меняются.

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
mod transaction;
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
pub use forgedb_sstable::CacheStats;
pub use recovery_plan::RecoveryPlan;
pub use snapshot::Snapshot;
pub use transaction::{Transaction, TransactionState};
pub use verify::{VerifyOptions, VerifyReport};
pub use write_options::{Durability, WriteOptions};
