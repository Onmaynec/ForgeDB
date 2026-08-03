//! Публичный API ForgeDB v0.1.0.
//!
//! Версия предоставляет durable WAL V1, mutable memtable, point get/put/delete,
//! атомарный WriteBatch и восстановление после повторного открытия.

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
