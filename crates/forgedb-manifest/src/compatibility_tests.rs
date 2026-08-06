use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb_core::{DatabaseId, Error, Limits, Result};
use forgedb_fs::NativeFileSystem;

use crate::checksum::crc32c;
use crate::codec::encode_edit_frame;
use crate::{
    CURRENT_FILE_NAME, MANIFEST_HEADER_SIZE, ManifestState, manifest_file_name, open_manifest,
};

fn temporary_directory(name: &str) -> Result<PathBuf> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-manifest-compat-{name}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
    Ok(path)
}

fn write_v1_manifest(directory: &PathBuf, database_id: DatabaseId, limits: &Limits) -> Result<()> {
    let state = ManifestState::initial(database_id, 1)?;
    let name = manifest_file_name(1);
    let mut header = [0_u8; MANIFEST_HEADER_SIZE];
    header[0..8].copy_from_slice(b"FORGEMAN");
    header[8..10].copy_from_slice(&1_u16.to_le_bytes());
    header[10..12].copy_from_slice(&(MANIFEST_HEADER_SIZE as u16).to_le_bytes());
    header[16..32].copy_from_slice(database_id.as_bytes());
    header[32..40].copy_from_slice(&1_u64.to_le_bytes());
    header[48..52].copy_from_slice(&1_u32.to_le_bytes());
    let checksum = crc32c(&header[..60]);
    header[60..64].copy_from_slice(&checksum.to_le_bytes());
    let frame = encode_edit_frame(&state.snapshot_edit(), limits, 1)?;
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(&frame);
    fs::write(directory.join(&name), bytes)
        .map_err(|source| Error::io("write_v1_manifest", Some(directory.join(&name)), source))?;
    fs::write(directory.join(CURRENT_FILE_NAME), format!("{name}\n")).map_err(|source| {
        Error::io(
            "write_v1_current",
            Some(directory.join(CURRENT_FILE_NAME)),
            source,
        )
    })?;
    Ok(())
}

#[test]
fn read_only_open_keeps_manifest_v1_authoritative() -> Result<()> {
    let directory = temporary_directory("read-only")?;
    let database_id = DatabaseId::random();
    let limits = Limits::default();
    write_v1_manifest(&directory, database_id, &limits)?;

    let opened = open_manifest(&NativeFileSystem, &directory, database_id, &limits, true)?;
    assert_eq!(opened.replay.format_version, 1);
    assert!(opened.writer.is_none());
    assert_eq!(
        fs::read_to_string(directory.join(CURRENT_FILE_NAME))
            .ok()
            .as_deref(),
        Some("MANIFEST-000001\n")
    );
    assert!(!directory.join(manifest_file_name(2)).exists());

    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn writable_open_rotates_v1_to_new_v2_manifest() -> Result<()> {
    let directory = temporary_directory("upgrade")?;
    let database_id = DatabaseId::random();
    let limits = Limits::default();
    write_v1_manifest(&directory, database_id, &limits)?;

    let opened = open_manifest(&NativeFileSystem, &directory, database_id, &limits, false)?;
    assert_eq!(opened.replay.format_version, 2);
    assert_eq!(opened.state.manifest_number, 2);
    assert_eq!(opened.state.next_file_number, 3);
    assert_eq!(
        opened.writer.as_ref().map(|writer| writer.format_version()),
        Some(2)
    );
    assert!(directory.join(manifest_file_name(1)).exists());
    assert!(directory.join(manifest_file_name(2)).exists());
    assert_eq!(
        fs::read_to_string(directory.join(CURRENT_FILE_NAME))
            .ok()
            .as_deref(),
        Some("MANIFEST-000002\n")
    );

    drop(opened);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}
