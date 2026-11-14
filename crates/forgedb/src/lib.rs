//! Публичный API ForgeDB v0.9.0.
//!
//! WAL/LSM storage core с snapshot isolation, индексными SST reads, bounded
//! decoded-block cache, lazy SST readers, безопасной очисткой retired SST и повторяемым close.
//! Default persisted WAL/SSTable/MANIFEST форматы не меняются. Каждая именованная
//! Column Family имеет собственный локальный WAL/LSM и registry V1.
//! FQL V1 реализован в `forgedb-cli` поверх публичных API; persisted format не меняется.

mod database_v3_fixed {
    include!("database_v3_fixed.rs");
    include!("database_streaming_wrapper.rs");
    include!("compaction.rs");
}
mod column_families;
mod entry_cursor;
mod inventory;
mod iterator;
mod mvcc_memtable;
mod obsolete;
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

pub use column_families::{ColumnFamilies, ColumnFamilyInfo};
pub use database_v3_fixed::StreamingDatabase as Database;
pub use forgedb_core::{
    BatchOperation, ColumnFamilyId, DatabaseId, Error, InternalKey, InternalKeyCodec,
    InternalKeyComparator, Limits, Result, SequenceNumber, ValueType, WriteBatch,
};
pub use forgedb_sstable::{CacheStats, TableHandleStats};
pub use inventory::FileInventory;
pub use iterator::DbIterator;
pub use obsolete::CleanupReport;
pub use options::Options;
pub use read_options::ReadOptions;
pub use recovery_plan::RecoveryPlan;
pub use snapshot::Snapshot;
pub use transaction::{Transaction, TransactionState};
pub use verify::{VerifyOptions, VerifyReport};
pub use write_options::{Durability, WriteOptions};
