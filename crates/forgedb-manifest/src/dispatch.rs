use std::io::ErrorKind;
use std::path::Path;

use forgedb_core::{DatabaseId, Error, Limits, Result};
use forgedb_fs::FileSystem;
use forgedb_sstable::sst_file_name;
use forgedb_wal::wal_file_name;

use crate::log::{OpenManifest, open_manifest};
use crate::manifest_file_name;
use crate::state::ManifestState;
use crate::v2::{MANIFEST_V2_FORMAT_VERSION, ManifestStateV2};
use crate::v2_log::{OpenManifestV2, open_manifest_v2};

/// Active MANIFEST, определённый по persisted header version.
#[derive(Debug)]
pub enum OpenManifestAny<W: forgedb_fs::WritableFile> {
    V1(OpenManifest<W>),
    V2(OpenManifestV2<W>),
}

impl<W: forgedb_fs::WritableFile> OpenManifestAny<W> {
    #[must_use]
    pub const fn format_version(&self) -> u16 {
        match self {
            Self::V1(_) => 1,
            Self::V2(_) => MANIFEST_V2_FORMAT_VERSION,
        }
    }
}

/// Открывает V1 или V2, но не интерпретирует future version как известную.
pub fn open_manifest_any<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    limits: &Limits,
    read_only: bool,
) -> Result<OpenManifestAny<F::WritableFile>> {
    match open_manifest_v2(filesystem, directory, database_id, limits, read_only) {
        Ok(opened) => Ok(OpenManifestAny::V2(opened)),
        Err(Error::UnsupportedFormat {
            component: "MANIFEST V2",
            found: 1,
            ..
        }) => open_manifest(
            filesystem,
            directory,
            database_id,
            limits,
            read_only,
        )
        .map(OpenManifestAny::V1),
        Err(error) => Err(error),
    }
}

/// Создаёт только in-memory V2 view старого V1 state без потребления file number.
pub fn bridge_manifest_v1_to_v2(
    v1: &ManifestState,
    level_count: usize,
    limits: &Limits,
) -> Result<ManifestStateV2> {
    v1.validate(limits)?;
    let mut state = ManifestStateV2::initial(
        v1.database_id,
        v1.manifest_number,
        v1.log_number,
        level_count,
    )?;
    state.comparator = v1.comparator.clone();
    state.previous_log_number = v1.previous_log_number;
    state.next_file_number = v1.next_file_number;
    state.last_sequence = v1.last_sequence;
    let level_zero = state.levels.first_mut().ok_or(Error::InternalInvariant {
        invariant: "I-MANIFEST-V1-BRIDGE-L0",
        reason: "level-aware bridge не содержит L0".to_owned(),
    })?;
    *level_zero = v1.l0_files.clone();
    state.validate(limits)?;
    Ok(state)
}

/// Выбирает первый свободный unified file number для V1→V2 migration.
///
/// Проверяются canonical MANIFEST, MANIFEST temp, WAL и SST имена. Найденные
/// orphan files не удаляются и не перезаписываются.
pub fn next_available_manifest_number<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    minimum: u64,
) -> Result<u64> {
    if minimum == 0 {
        return Err(Error::invalid_argument(
            "minimum_manifest_number",
            "минимальный file number не может быть нулём",
        ));
    }
    let mut candidate = minimum;
    loop {
        let manifest = manifest_file_name(candidate);
        let paths = [
            directory.join(&manifest),
            directory.join(format!("{manifest}.tmp")),
            directory.join(wal_file_name(candidate)),
            directory.join(sst_file_name(candidate)),
        ];
        let mut occupied = false;
        for path in &paths {
            if path_exists(filesystem, path)? {
                occupied = true;
                break;
            }
        }
        if !occupied {
            return Ok(candidate);
        }
        candidate = candidate.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "file_number",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    }
}

fn path_exists<F: FileSystem>(filesystem: &F, path: &Path) -> Result<bool> {
    match filesystem.file_size(path) {
        Ok(_) => Ok(true),
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

    use super::*;
    use crate::{create_manifest, migrate_manifest_v1_to_v2};

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-manifest-dispatch-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn dispatch_opens_v1_then_v2_without_future_fallback() -> Result<()> {
        let directory = temporary_directory("versions")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let (writer, v1) = create_manifest(
            &filesystem,
            &directory,
            database_id,
            1,
            &limits,
        )?;
        drop(writer);

        let opened_v1 = open_manifest_any(
            &filesystem,
            &directory,
            database_id,
            &limits,
            true,
        )?;
        assert_eq!(opened_v1.format_version(), 1);
        drop(opened_v1);

        let number =
            next_available_manifest_number(&filesystem, &directory, v1.next_file_number)?;
        let (writer, _) = migrate_manifest_v1_to_v2(
            &filesystem,
            &directory,
            &v1,
            number,
            4,
            &limits,
        )?;
        drop(writer);
        let opened_v2 = open_manifest_any(
            &filesystem,
            &directory,
            database_id,
            &limits,
            true,
        )?;
        assert_eq!(opened_v2.format_version(), MANIFEST_V2_FORMAT_VERSION);
        drop(opened_v2);

        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn allocator_skips_orphan_numbers_across_generated_file_types() -> Result<()> {
        let directory = temporary_directory("allocator")?;
        let filesystem = NativeFileSystem;
        fs::write(directory.join(manifest_file_name(5)), b"manifest")
            .map_err(|source| Error::io("write_test_manifest", Some(directory.clone()), source))?;
        fs::write(directory.join(wal_file_name(6)), b"wal")
            .map_err(|source| Error::io("write_test_wal", Some(directory.clone()), source))?;
        fs::write(directory.join(sst_file_name(7)), b"sst")
            .map_err(|source| Error::io("write_test_sst", Some(directory.clone()), source))?;
        fs::write(
            directory.join(format!("{}.tmp", manifest_file_name(8))),
            b"temporary",
        )
        .map_err(|source| Error::io("write_test_temp", Some(directory.clone()), source))?;

        assert_eq!(
            next_available_manifest_number(&filesystem, &directory, 5)?,
            9
        );
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn bridge_preserves_v1_authoritative_numbers() -> Result<()> {
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let state = ManifestState::initial(database_id, 1)?;
        let bridge = bridge_manifest_v1_to_v2(&state, 4, &limits)?;
        assert_eq!(bridge.manifest_number, state.manifest_number);
        assert_eq!(bridge.log_number, state.log_number);
        assert_eq!(bridge.next_file_number, state.next_file_number);
        assert_eq!(bridge.levels.len(), 4);
        Ok(())
    }
}
