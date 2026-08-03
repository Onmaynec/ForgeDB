//! Файловая абстракция ForgeDB, native-реализация, блокировка и database identity.
//!
//! Crate изолирует platform-specific I/O, синхронизацию каталогов,
//! усечение файлов и process-level lock от WAL и публичного API.

mod filesystem;
mod identity;
mod native;

pub use filesystem::{FileSystem, RandomAccessReader, SequentialReader, WritableFile};
pub use identity::{IDENTITY_FILE_NAME, load_or_create_identity, read_identity};
pub use native::{NativeFileSystem, NativeLockGuard};
