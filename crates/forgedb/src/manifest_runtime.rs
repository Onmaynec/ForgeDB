use std::fs::File;
use std::path::Path;

use forgedb_core::{DatabaseId, Error, Limits, Result};
use forgedb_fs::NativeFileSystem;
use forgedb_manifest::{
    ManifestStateV2, ManifestV2Writer, OpenManifestAny, bridge_manifest_v1_to_v2, create_manifest,
    migrate_manifest_v1_to_v2, next_available_manifest_number, open_manifest_any,
};

#[derive(Debug)]
pub(crate) struct RuntimeManifest {
    pub(crate) writer: Option<ManifestV2Writer<File>>,
    pub(crate) state: ManifestStateV2,
}

pub(crate) struct RuntimeManifestRequest<'a> {
    pub(crate) filesystem: &'a NativeFileSystem,
    pub(crate) directory: &'a Path,
    pub(crate) database_id: DatabaseId,
    pub(crate) limits: &'a Limits,
    pub(crate) read_only: bool,
    pub(crate) current_exists: bool,
    pub(crate) level_count: usize,
}

pub(crate) fn open_runtime_manifest(
    request: RuntimeManifestRequest<'_>,
) -> Result<RuntimeManifest> {
    let RuntimeManifestRequest {
        filesystem,
        directory,
        database_id,
        limits,
        read_only,
        current_exists,
        level_count,
    } = request;

    if !current_exists {
        if read_only {
            return Err(Error::UnsupportedFormat {
                component: "CURRENT/MANIFEST",
                found: 0,
                minimum_supported: 1,
                maximum_supported: 2,
            });
        }
        let (writer_v1, state_v1) = create_manifest(filesystem, directory, database_id, 1, limits)?;
        drop(writer_v1);
        return migrate_runtime_manifest(filesystem, directory, state_v1, level_count, limits);
    }

    match open_manifest_any(filesystem, directory, database_id, limits, read_only)? {
        OpenManifestAny::V2(opened) => Ok(RuntimeManifest {
            writer: opened.writer,
            state: opened.state,
        }),
        OpenManifestAny::V1(opened) if read_only => Ok(RuntimeManifest {
            writer: None,
            state: bridge_manifest_v1_to_v2(&opened.state, level_count, limits)?,
        }),
        OpenManifestAny::V1(opened) => {
            let state_v1 = opened.state;
            drop(opened.writer);
            migrate_runtime_manifest(filesystem, directory, state_v1, level_count, limits)
        }
    }
}

fn migrate_runtime_manifest(
    filesystem: &NativeFileSystem,
    directory: &Path,
    state_v1: forgedb_manifest::ManifestState,
    level_count: usize,
    limits: &Limits,
) -> Result<RuntimeManifest> {
    let manifest_number =
        next_available_manifest_number(filesystem, directory, state_v1.next_file_number)?;
    let (writer, state) = migrate_manifest_v1_to_v2(
        filesystem,
        directory,
        &state_v1,
        manifest_number,
        level_count,
        limits,
    )?;
    Ok(RuntimeManifest {
        writer: Some(writer),
        state,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_fs::FileSystem;
    use forgedb_manifest::{MANIFEST_V2_FORMAT_VERSION, open_manifest_any};

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-runtime-manifest-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn read_only_v1_open_does_not_publish_v2() -> Result<()> {
        let directory = temporary_directory("readonly-v1")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let (writer, state_v1) = create_manifest(&filesystem, &directory, database_id, 1, &limits)?;
        drop(writer);
        let current_before = fs::read(directory.join("CURRENT"))
            .map_err(|source| Error::io("read_current_before", Some(directory.clone()), source))?;

        let runtime = open_runtime_manifest(RuntimeManifestRequest {
            filesystem: &filesystem,
            directory: &directory,
            database_id,
            limits: &limits,
            read_only: true,
            current_exists: true,
            level_count: 4,
        })?;
        assert!(runtime.writer.is_none());
        assert_eq!(runtime.state.manifest_number, state_v1.manifest_number);
        assert_eq!(
            fs::read(directory.join("CURRENT")).map_err(|source| Error::io(
                "read_current_after",
                Some(directory.clone()),
                source
            ))?,
            current_before
        );
        drop(runtime);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn read_write_v1_open_publishes_v2_and_reopens_directly() -> Result<()> {
        let directory = temporary_directory("readwrite-v1")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let (writer, _) = create_manifest(&filesystem, &directory, database_id, 1, &limits)?;
        drop(writer);

        let migrated = open_runtime_manifest(RuntimeManifestRequest {
            filesystem: &filesystem,
            directory: &directory,
            database_id,
            limits: &limits,
            read_only: false,
            current_exists: true,
            level_count: 4,
        })?;
        assert!(migrated.writer.is_some());
        let migrated_number = migrated.state.manifest_number;
        drop(migrated);

        let direct = open_runtime_manifest(RuntimeManifestRequest {
            filesystem: &filesystem,
            directory: &directory,
            database_id,
            limits: &limits,
            read_only: true,
            current_exists: true,
            level_count: 4,
        })?;
        assert_eq!(direct.state.manifest_number, migrated_number);
        let opened = open_manifest_any(&filesystem, &directory, database_id, &limits, true)?;
        assert_eq!(opened.format_version(), MANIFEST_V2_FORMAT_VERSION);
        drop(opened);
        drop(direct);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn new_database_bootstraps_wal_then_publishes_v2() -> Result<()> {
        let directory = temporary_directory("new")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let runtime = open_runtime_manifest(RuntimeManifestRequest {
            filesystem: &filesystem,
            directory: &directory,
            database_id,
            limits: &limits,
            read_only: false,
            current_exists: false,
            level_count: 4,
        })?;
        assert!(runtime.writer.is_some());
        assert!(directory.join(forgedb_wal::wal_file_name(1)).exists());
        assert_eq!(
            filesystem.file_size(
                runtime
                    .writer
                    .as_ref()
                    .ok_or(Error::InternalInvariant {
                        invariant: "I-RUNTIME-MANIFEST-TEST-WRITER",
                        reason: "new runtime manifest не содержит writer".to_owned(),
                    })?
                    .path()
            )?,
            runtime
                .writer
                .as_ref()
                .ok_or(Error::InternalInvariant {
                    invariant: "I-RUNTIME-MANIFEST-TEST-WRITER",
                    reason: "new runtime manifest не содержит writer".to_owned(),
                })?
                .bytes_written()
        );
        drop(runtime);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
