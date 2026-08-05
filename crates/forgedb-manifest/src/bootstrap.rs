use std::io::ErrorKind;
use std::path::Path;

use forgedb_core::{DatabaseId, Error, Limits, Result};
use forgedb_fs::FileSystem;
use forgedb_wal::create_wal;

use crate::log::{ManifestWriter, create_manifest as create_manifest_file};
use crate::state::ManifestState;
use crate::{CURRENT_FILE_NAME, INITIAL_MANIFEST_NUMBER, manifest_file_name};

/// Создаёт initial WAL до публикации MANIFEST/CURRENT.
///
/// После появления `CURRENT` recovery всегда может требовать существование указанного
/// current WAL и не нуждается в небезопасном молчаливом пересоздании файла.
pub fn create_manifest<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    initial_log_number: u64,
    limits: &Limits,
) -> Result<(ManifestWriter<F::WritableFile>, ManifestState)> {
    if path_exists(filesystem, &directory.join(CURRENT_FILE_NAME))? {
        return Err(Error::invalid_argument(
            "directory",
            "CURRENT уже существует; initial MANIFEST нельзя пересоздавать",
        ));
    }

    let bootstrap_wal = create_wal(filesystem, directory, database_id, initial_log_number)?;
    cleanup_bootstrap_orphans(filesystem, directory)?;
    let result = create_manifest_file(
        filesystem,
        directory,
        database_id,
        initial_log_number,
        limits,
    );
    drop(bootstrap_wal);
    result
}

fn cleanup_bootstrap_orphans<F: FileSystem>(filesystem: &F, directory: &Path) -> Result<()> {
    let manifest_name = manifest_file_name(INITIAL_MANIFEST_NUMBER);
    let candidates = [
        directory.join(&manifest_name),
        directory.join(format!("{manifest_name}.tmp")),
        directory.join("CURRENT.tmp"),
    ];
    let mut removed = false;
    for path in candidates {
        removed |= remove_if_exists(filesystem, &path)?;
    }
    if removed {
        filesystem.sync_directory(directory)?;
    }
    Ok(())
}

fn path_exists<F: FileSystem>(filesystem: &F, path: &Path) -> Result<bool> {
    match filesystem.open_sequential(path) {
        Ok(_) => Ok(true),
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn remove_if_exists<F: FileSystem>(filesystem: &F, path: &Path) -> Result<bool> {
    match filesystem.remove_file(path) {
        Ok(()) => Ok(true),
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_fs::NativeFileSystem;
    use forgedb_wal::wal_file_name;

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-manifest-bootstrap-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn current_is_published_with_existing_initial_wal() -> Result<()> {
        let directory = temporary_directory("initial")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let (writer, _) = create_manifest(&filesystem, &directory, database_id, 1, &limits)?;
        drop(writer);
        assert!(directory.join(wal_file_name(1)).exists());
        assert!(directory.join(CURRENT_FILE_NAME).exists());
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn bootstrap_reuses_header_only_wal_after_crash() -> Result<()> {
        let directory = temporary_directory("retry")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        drop(create_wal(&filesystem, &directory, database_id, 1)?);
        assert!(!directory.join(CURRENT_FILE_NAME).exists());
        let (writer, _) = create_manifest(&filesystem, &directory, database_id, 1, &limits)?;
        drop(writer);
        assert!(directory.join(CURRENT_FILE_NAME).exists());
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn bootstrap_removes_only_reserved_orphans_without_current() -> Result<()> {
        let directory = temporary_directory("orphan-files")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let manifest_name = manifest_file_name(INITIAL_MANIFEST_NUMBER);
        fs::write(directory.join(&manifest_name), b"orphan").map_err(|source| {
            Error::io("write_orphan_manifest", Some(directory.clone()), source)
        })?;
        fs::write(directory.join(format!("{manifest_name}.tmp")), b"orphan").map_err(|source| {
            Error::io(
                "write_orphan_manifest_temp",
                Some(directory.clone()),
                source,
            )
        })?;
        fs::write(directory.join("CURRENT.tmp"), b"orphan").map_err(|source| {
            Error::io("write_orphan_current_temp", Some(directory.clone()), source)
        })?;
        fs::write(directory.join("user.data"), b"keep")
            .map_err(|source| Error::io("write_user_file", Some(directory.clone()), source))?;

        let (writer, _) = create_manifest(&filesystem, &directory, database_id, 1, &limits)?;
        drop(writer);
        assert!(directory.join(CURRENT_FILE_NAME).exists());
        assert!(directory.join("user.data").exists());
        assert!(!directory.join(format!("{manifest_name}.tmp")).exists());
        assert!(!directory.join("CURRENT.tmp").exists());
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn existing_current_prevents_reinitialization() -> Result<()> {
        let directory = temporary_directory("existing-current")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let (writer, _) = create_manifest(&filesystem, &directory, database_id, 1, &limits)?;
        drop(writer);
        assert!(matches!(
            create_manifest(&filesystem, &directory, database_id, 1, &limits),
            Err(Error::InvalidArgument { .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
