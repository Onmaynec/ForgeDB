use std::io::{ErrorKind, Read, Write};
use std::path::Path;

use forgedb_core::{DatabaseId, Error, Result};

use crate::{FileSystem, WritableFile};

/// Стабильное имя файла database identity.
pub const IDENTITY_FILE_NAME: &str = "IDENTITY";
const IDENTITY_PREFIX: &str = "FORGEDB-IDENTITY-V1\n";
const MAX_IDENTITY_BYTES: u64 = 128;

/// Загружает существующий database identity или устанавливает новый случайный identity.
///
/// Caller обязан удерживать `LOCK` базы до вызова этой операции.
pub fn load_or_create_identity<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
) -> Result<DatabaseId> {
    filesystem.create_dir_all(directory)?;
    let path = directory.join(IDENTITY_FILE_NAME);
    match filesystem.open_sequential(&path) {
        Ok(reader) => read_identity_from_reader(reader),
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            create_identity(filesystem, directory, &path)
        }
        Err(error) => Err(error),
    }
}

/// Читает и полностью проверяет существующий файл identity.
pub fn read_identity<F: FileSystem>(filesystem: &F, directory: &Path) -> Result<DatabaseId> {
    let path = directory.join(IDENTITY_FILE_NAME);
    let reader = filesystem.open_sequential(&path)?;
    read_identity_from_reader(reader)
}

fn create_identity<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    path: &Path,
) -> Result<DatabaseId> {
    let identity = DatabaseId::random();
    let contents = format!("{IDENTITY_PREFIX}{identity}\n");
    match filesystem.create_new_writable(path) {
        Ok(mut file) => {
            file.write_all(contents.as_bytes())
                .map_err(|source| Error::io("write_identity", Some(path.into()), source))?;
            file.sync_all()
                .map_err(|source| Error::io("sync_identity", Some(path.into()), source))?;
            drop(file);
            filesystem.sync_directory(directory)?;
            Ok(identity)
        }
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists => {
            read_identity(filesystem, directory)
        }
        Err(error) => Err(error),
    }
}

fn read_identity_from_reader<R: Read>(reader: R) -> Result<DatabaseId> {
    let mut bytes = Vec::new();
    let mut bounded = reader.take(MAX_IDENTITY_BYTES + 1);
    bounded
        .read_to_end(&mut bytes)
        .map_err(|source| Error::io("read_identity", None, source))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_IDENTITY_BYTES {
        return Err(Error::ResourceLimit {
            resource: "identity_file_size",
            limit: MAX_IDENTITY_BYTES,
            observed: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Error::corruption("database identity", "файл не является корректным UTF-8"))?;
    let identifier = text
        .strip_prefix(IDENTITY_PREFIX)
        .and_then(|rest| rest.strip_suffix('\n'))
        .ok_or_else(|| {
            if text.starts_with("FORGEDB-IDENTITY-V") {
                Error::UnsupportedFormat {
                    component: "database identity",
                    found: parse_version(text).unwrap_or(u32::MAX),
                    minimum_supported: 1,
                    maximum_supported: 1,
                }
            } else {
                Error::corruption(
                    "database identity",
                    "заголовок или завершающий перевод строки неверны",
                )
            }
        })?;
    DatabaseId::parse(identifier)
}

fn parse_version(text: &str) -> Option<u32> {
    let tail = text.strip_prefix("FORGEDB-IDENTITY-V")?;
    let digits = tail.split_once('\n')?.0;
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::NativeFileSystem;

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
    fn identity_is_stable_across_loads() -> Result<()> {
        let directory = temporary_directory("identity")?;
        let filesystem = NativeFileSystem;
        let first = load_or_create_identity(&filesystem, &directory)?;
        let second = load_or_create_identity(&filesystem, &directory)?;
        assert_eq!(first, second);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn future_identity_version_is_rejected() -> Result<()> {
        let directory = temporary_directory("identity-future")?;
        let path = directory.join(IDENTITY_FILE_NAME);
        fs::write(
            &path,
            b"FORGEDB-IDENTITY-V2\n00000000-0000-4000-8000-000000000000\n",
        )
        .map_err(|source| Error::io("write_test_identity", Some(path), source))?;
        assert!(matches!(
            read_identity(&NativeFileSystem, &directory),
            Err(Error::UnsupportedFormat { found: 2, .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
