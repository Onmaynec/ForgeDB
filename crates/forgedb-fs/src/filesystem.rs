use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use forgedb_core::Result;

/// Sequential readable file contract.
pub trait SequentialReader: Read + Send {}

impl<T> SequentialReader for T where T: Read + Send {}

/// Random-access readable file contract used by future table readers.
pub trait RandomAccessReader: Read + Seek + Send + Sync {}

impl<T> RandomAccessReader for T where T: Read + Seek + Send + Sync {}

/// Writable and synchronizable file contract.
pub trait WritableFile: Write + Seek + Send {
    /// Synchronizes file data and metadata required by the platform.
    fn sync_all(&self) -> std::io::Result<()>;

    /// Synchronizes file data where the platform distinguishes data and metadata.
    fn sync_data(&self) -> std::io::Result<()>;
}

impl WritableFile for std::fs::File {
    fn sync_all(&self) -> std::io::Result<()> {
        std::fs::File::sync_all(self)
    }

    fn sync_data(&self) -> std::io::Result<()> {
        std::fs::File::sync_data(self)
    }
}

/// Filesystem operations required by persisted ForgeDB protocols.
pub trait FileSystem: Send + Sync + 'static {
    /// Sequential reader type.
    type SequentialReader: SequentialReader;
    /// Random-access reader type.
    type RandomAccessReader: RandomAccessReader;
    /// Writable file type.
    type WritableFile: WritableFile;
    /// Exclusive database lock guard type.
    type LockGuard: Send + Sync;

    /// Creates a directory and missing parents.
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    /// Opens a file for sequential reads.
    fn open_sequential(&self, path: &Path) -> Result<Self::SequentialReader>;
    /// Opens a file for random-access reads.
    fn open_random_access(&self, path: &Path) -> Result<Self::RandomAccessReader>;
    /// Creates or truncates a writable file.
    fn create_writable(&self, path: &Path) -> Result<Self::WritableFile>;
    /// Creates a writable file only when it does not already exist.
    fn create_new_writable(&self, path: &Path) -> Result<Self::WritableFile>;
    /// Opens or creates a writable file for append.
    fn open_appendable(&self, path: &Path) -> Result<Self::WritableFile>;
    /// Renames one path using the platform's native rename primitive.
    fn rename_atomic(&self, from: &Path, to: &Path) -> Result<()>;
    /// Removes one regular file.
    fn remove_file(&self, path: &Path) -> Result<()>;
    /// Lists direct children of a directory.
    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>>;
    /// Returns the current file size.
    fn file_size(&self, path: &Path) -> Result<u64>;
    /// Synchronizes directory metadata where supported.
    fn sync_directory(&self, path: &Path) -> Result<()>;
    /// Acquires a non-blocking exclusive process-level lock.
    fn lock_exclusive(&self, path: &Path) -> Result<Self::LockGuard>;
}
