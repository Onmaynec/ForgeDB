//! Публичный API ForgeDB v0.3.0.
//!
//! Версия добавляет immutable VersionSet, MVCC snapshots, snapshot-aware point reads,
//! forward range scan, file inventory и deep verification поверх WAL/SSTable/MANIFEST V1.

mod database_v3_fixed;
mod inventory;
mod iterator;
mod mvcc_memtable;
mod options;
mod read_options;
mod snapshot;
mod table_scan;
mod verify;
mod version_set;
mod write_options;

pub use database_v3_fixed::Database;
pub use forgedb_core::{
    BatchOperation, ColumnFamilyId, DatabaseId, Error, InternalKey, InternalKeyCodec,
    InternalKeyComparator, Limits, Result, SequenceNumber, ValueType, WriteBatch,
};
pub use inventory::FileInventory;
pub use iterator::DbIterator;
pub use options::Options;
pub use read_options::ReadOptions;
pub use snapshot::Snapshot;
pub use verify::{VerifyOptions, VerifyReport};
pub use write_options::{Durability, WriteOptions};
