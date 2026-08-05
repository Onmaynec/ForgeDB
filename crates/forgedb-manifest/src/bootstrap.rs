use std::path::Path;

use forgedb_core::{DatabaseId, Limits, Result};
use forgedb_fs::FileSystem;
use forgedb_wal::create_wal;

use crate::log::{ManifestWriter, create_manifest as create_manifest_file};
use crate::state::ManifestState;

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
    let bootstrap_wal = create_wal(
        filesystem,
        directory,
        database_id,
        initial_log_number,
    )?;
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_core::Error;
    use forgedb_fs::NativeFileSystem;
    use forgedb_wal::wal_file_name;

    use super::*;
    use crate::CURRENT_FILE_NAME;

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
        let (writer, _) = create_manifest(
            &filesystem,
            &directory,
            database_id,
            1,
            &limits,
        )?;
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
        let (writer, _) = create_manifest(
            &filesystem,
            &directory,
            database_id,
            1,
            &limits,
        )?;
        drop(writer);
        assert!(directory.join(CURRENT_FILE_NAME).exists());
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
