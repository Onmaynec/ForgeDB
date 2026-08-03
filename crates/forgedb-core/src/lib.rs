//! Storage-neutral примитивы ForgeDB.
//!
//! Crate содержит модель ошибок, bounded limits, идентификаторы, sequence numbers,
//! internal keys и атомарный `WriteBatch`. Он не зависит от файловой системы,
//! WAL или публичной orchestration базы.

mod batch;
mod error;
mod ids;
mod internal_key;
mod limits;

pub use batch::{BatchOperation, WriteBatch};
pub use error::{Error, Result};
pub use ids::{ColumnFamilyId, DatabaseId, SequenceNumber, ValueType};
pub use internal_key::{InternalKey, InternalKeyCodec, InternalKeyComparator};
pub use limits::Limits;
