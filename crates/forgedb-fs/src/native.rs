use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use forgedb_core::{Error, Result};

use crate::FileSystem;

/// Safe native filesystem implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFileSystem;

/// Guard that keeps an operating-system exclusive file lock alive.
#[derive(Debug)]
pub struct NativeLockGuard {
    file: File,
    path: PathBuf,
}

impl NativeLockGuard {
    /// Returns the lock-file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the locked file handle for diagnostics.
    #[must_use]
    pub fn file(&self) -> &File {
        &self.file
    }
}

impl FileSystem for NativeFileSystem {
    type SequentialReader = File;
    type RandomAccessReader = File;
    type WritableFile = File;
    type LockGuard = NativeLockGuard;

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path)
            .map_err(|source| Error::io("create_dir_all", Some(path.into()), source))
    }

    fn open_sequential(&self, path: &Path) -> Result<Self::SequentialReader> {
        File::open(path).map_err(|source| Error::io("open_sequential", Some(path.into()), source))
    }

    fn open_random_access(&self, path: &Path) -> Result<Self::RandomAccessReader> {
        File::open(path)
            .map_err(|source| Error::io("open_random_access", Some(path.into()), source))
    }

    fn create_writable(&self, path: &Path) -> Result<Self::WritableFile> {
        File::create(path).map_err(|source| Error::io("create_writable", Some(path.into()), source))
    }

    fn create_new_writable(&self, path: &Path) -> Result<Self::WritableFile> {
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|source| Error::io("create_new_writable", Some(path.into()), source))
    }

    fn open_appendable(&self, path: &Path) -> Result<Self::WritableFile> {
        OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)
            .map_err(|source| Error::io("open_appendable", Some(path.into()), source))
    }

    fn rename_atomic(&self, from: &Path, to: &Path) -> Result<()> {
        fs::rename(from, to).map_err(|source| {
            Error::io(
                "rename_atomic",
                Some(PathBuf::from(format!(
                    "{} -> {}",
                    from.display(),
                    to.display()
                ))),
                source,
            )
        })
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        fs::remove_file(path).map_err(|source| Error::io("remove_file", Some(path.into()), source))
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let entries = fs::read_dir(path)
            .map_err(|source| Error::io("list_dir", Some(path.into()), source))?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|source| Error::io("list_dir_entry", Some(path.into()), source))?;
            paths.push(entry.path());
        }
        paths.sort();
        Ok(paths)
    }

    fn file_size(&self, path: &Path) -> Result<u64> {
        fs::metadata(path)
            .map(|metadata| metadata.len())
            .map_err(|source| Error::io("file_size", Some(path.into()), source))
    }

    fn sync_directory(&self, path: &Path) -> Result<()> {
        sync_directory_native(path)
            .map_err(|source| Error::io("sync_directory", Some(path.into()), source))
    }

    fn lock_exclusive(&self, path: &Path) -> Result<Self::LockGuard> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|source| Error::io("open_lock_file", Some(path.into()), source))?;
        match file.try_lock() {
            Ok(()) => Ok(NativeLockGuard {
                file,
                path: path.to_path_buf(),
            }),
            Err(TryLockError::WouldBlock) => Err(Error::Busy {
                reason: format!("database lock is already held at {}", path.display()),
            }),
            Err(TryLockError::Error(source)) => {
                Err(Error::io("lock_exclusive", Some(path.into()), source))
            }
        }
    }
}

#[cfg(unix)]
fn sync_directory_native(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory_native(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

#[cfg(not(any(unix, windows)))]
fn sync_directory_native(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("forgedb-{name}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn second_exclusive_lock_is_rejected() -> Result<()> {
        let directory = temporary_directory("lock")?;
        let path = directory.join("LOCK");
        let filesystem = NativeFileSystem;
        let first = filesystem.lock_exclusive(&path)?;
        assert!(matches!(
            filesystem.lock_exclusive(&path),
            Err(Error::Busy { .. })
        ));
        drop(first);
        let second = filesystem.lock_exclusive(&path)?;
        drop(second);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
