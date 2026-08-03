//! Public ForgeDB API foundation.
//!
//! Version 0.0.1 intentionally exposes configuration and storage-neutral primitives only. It does
//! not yet expose a database read/write API or make durability claims.

mod options;

pub use forgedb_core::{
    BatchOperation, ColumnFamilyId, DatabaseId, Error, InternalKey, InternalKeyCodec,
    InternalKeyComparator, Limits, Result, SequenceNumber, ValueType, WriteBatch,
};
pub use options::Options;
