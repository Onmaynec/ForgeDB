//! Публичный API ForgeDB v0.2.0.
//!
//! Версия предоставляет durable WAL V1, mutable/immutable memtable, SSTable V1,
//! MANIFEST V1, crash-safe flush, point get/put/delete, атомарный WriteBatch
//! и восстановление после повторного открытия.

#[path = "database_v2_fixed.rs"]
mod database;
mod memtable;
mod options;
mod write_options;

pub use database::Database;
pub use forgedb_core::{
    BatchOperation, ColumnFamilyId, DatabaseId, Error, InternalKey, InternalKeyCodec,
    InternalKeyComparator, Limits, Result, SequenceNumber, ValueType, WriteBatch,
};
pub use options::Options;
pub use write_options::{Durability, WriteOptions};
