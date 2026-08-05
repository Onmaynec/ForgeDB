use std::io::ErrorKind;
use std::path::Path;

use forgedb_core::{DatabaseId, Error, Limits, Result};
use forgedb_fs::FileSystem;

use crate::log::{
    WalWriter, create_wal as create_new_wal, open_wal_numbered, replay_wal_numbered,
    wal_file_name,
};

/// Создаёт новое WAL-поколение или повторно открывает только строго проверенный пустой файл.
///
/// Идемпотентность нужна после crash между созданием нового WAL и публикацией
/// соответствующего MANIFEST edit. WAL с полным или усечённым кадром никогда не удаляется
/// и не принимается как безопасный orphan.
pub fn create_wal<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    file_number: u64,
) -> Result<WalWriter<F::WritableFile>> {
    if file_number == 0 {
        return Err(Error::invalid_argument(
            "file_number",
            "ноль зарезервирован",
        ));
    }
    let path = directory.join(wal_file_name(file_number));
    match filesystem.open_sequential(&path) {
        Ok(_) => {
            let limits = Limits::default();
            let summary = replay_wal_numbered(
                filesystem,
                &path,
                database_id,
                file_number,
                &limits,
                |_batch| {
                    Err(Error::corruption(
                        "orphan WAL",
                        "существующее WAL-поколение содержит complete frame",
                    ))
                },
            )?;
            if summary.truncated_tail || summary.frame_count != 0 {
                return Err(Error::corruption(
                    "orphan WAL",
                    "повторно использовать можно только header-only WAL без tail",
                ));
            }
            let (writer, reopened) = open_wal_numbered(
                filesystem,
                directory,
                database_id,
                file_number,
                &limits,
                |_batch| {
                    Err(Error::corruption(
                        "orphan WAL",
                        "header-only WAL неожиданно содержит frame при повторной проверке",
                    ))
                },
            )?;
            if reopened.truncated_tail || reopened.frame_count != 0 {
                return Err(Error::corruption(
                    "orphan WAL",
                    "WAL изменился между двумя проверками",
                ));
            }
            Ok(writer)
        }
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            create_new_wal(filesystem, directory, database_id, file_number)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_core::{SequenceNumber, WriteBatch};
    use forgedb_fs::NativeFileSystem;

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-wal-bootstrap-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn zero_generation_is_rejected() -> Result<()> {
        let directory = temporary_directory("zero")?;
        let filesystem = NativeFileSystem;
        assert!(matches!(
            create_wal(&filesystem, &directory, DatabaseId::random(), 0),
            Err(Error::InvalidArgument { .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn header_only_generation_is_idempotent() -> Result<()> {
        let directory = temporary_directory("empty")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        drop(create_wal(&filesystem, &directory, database_id, 7)?);
        drop(create_wal(&filesystem, &directory, database_id, 7)?);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn nonempty_generation_is_never_reused() -> Result<()> {
        let directory = temporary_directory("nonempty")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let mut writer = create_wal(&filesystem, &directory, database_id, 8)?;
        let mut batch = WriteBatch::new();
        batch.put(b"key", b"value")?;
        writer.append_batch(SequenceNumber::FIRST, &batch, &limits, true)?;
        drop(writer);
        assert!(matches!(
            create_wal(&filesystem, &directory, database_id, 8),
            Err(Error::Corruption { .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
