use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb_core::{DatabaseId, Error, Limits, Result};
use forgedb_fs::NativeFileSystem;

use crate::replay_manifest;

const V1_FIXTURE: &str = include_str!("../fixtures/manifest-v1-minimal.hex");

#[test]
fn fixed_v1_fixture_replays_after_v2_contract_is_added() -> Result<()> {
    let directory = temporary_directory("v1-fixture")?;
    let path = directory.join("MANIFEST-000001");
    fs::write(&path, decode_hex(V1_FIXTURE)?)
        .map_err(|source| Error::io("write_manifest_fixture", Some(path.clone()), source))?;
    let database_id =
        DatabaseId::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    let (state, summary) =
        replay_manifest(&NativeFileSystem, &path, database_id, 1, &Limits::default())?;
    assert_eq!(state.database_id, database_id);
    assert_eq!(state.manifest_number, 1);
    assert_eq!(state.log_number, 1);
    assert_eq!(state.next_file_number, 2);
    assert!(state.l0_files.is_empty());
    assert_eq!(summary.edit_count, 1);
    assert!(!summary.truncated_tail);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

fn decode_hex(input: &str) -> Result<Vec<u8>> {
    let compact = input.split_whitespace().collect::<String>();
    if compact.len() % 2 != 0 {
        return Err(Error::corruption(
            "MANIFEST V1 fixture",
            "hex fixture имеет нечётную длину",
        ));
    }
    compact
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair)
                .map_err(|_| Error::corruption("MANIFEST V1 fixture", "hex fixture не ASCII"))?;
            u8::from_str_radix(text, 16).map_err(|_| {
                Error::corruption("MANIFEST V1 fixture", "hex fixture содержит не-hex byte")
            })
        })
        .collect()
}

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
