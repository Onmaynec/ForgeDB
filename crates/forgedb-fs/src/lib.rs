//! ForgeDB filesystem abstraction, native implementation, locking, and identity.

mod filesystem;
mod identity;
mod native;

pub use filesystem::{FileSystem, RandomAccessReader, SequentialReader, WritableFile};
pub use identity::{IDENTITY_FILE_NAME, load_or_create_identity, read_identity};
pub use native::{NativeFileSystem, NativeLockGuard};
