use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{ColumnFamilies, ColumnFamilyId, Database, Options, WriteOptions};

fn root(name: &str) -> std::io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-v100-cli-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&path)?;
    Ok(path)
}

fn cli(args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_forgedb"))
        .args(args)
        .output()
}

fn message(output: &Output) -> String {
    format!(
        "stdout: {}, stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn as_text(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}

#[test]
fn version_readonly_doctor_and_missing_get_never_create_database()
-> Result<(), Box<dyn std::error::Error>> {
    let root = root("doctor")?;
    let missing = root.join("nonexistent");
    let version = cli(&["version"])?;
    assert!(version.status.success(), "{}", message(&version));
    assert_eq!(version.stdout, b"ForgeDB 1.0.0\n");

    let nonexistent_get = cli(&["get", as_text(&missing), "key"])?;
    assert!(!nonexistent_get.status.success());
    assert!(!missing.exists());

    let nonexistent_scan = cli(&["scan", as_text(&missing)])?;
    assert!(!nonexistent_scan.status.success());
    assert!(!missing.exists());

    let nonexistent_doctor = cli(&["doctor", as_text(&missing), "--deep"])?;
    assert!(!nonexistent_doctor.status.success());
    assert!(!missing.exists());

    let path = root.join("database");
    let database = Database::open(&path, Options::default())?;
    database.put(b"key", b"value", WriteOptions::sync())?;
    database.flush()?;
    database.close()?;
    drop(database);

    let before = fs::read_dir(&path)?
        .map(|entry| entry.map(|value| value.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    let doctor = cli(&["doctor", as_text(&path), "--deep"])?;
    assert!(doctor.status.success(), "{}", message(&doctor));
    assert!(String::from_utf8_lossy(&doctor.stdout).contains("status: OK"));
    assert!(String::from_utf8_lossy(&doctor.stdout).contains("verification: deep"));

    let after = fs::read_dir(&path)?
        .map(|entry| entry.map(|value| value.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(before, after);
    assert_eq!(cli(&["get", as_text(&path), "key"])?.stdout, b"value\n");

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn export_restore_roundtrip_all_families_binary_and_large_values()
-> Result<(), Box<dyn std::error::Error>> {
    let root = root("roundtrip")?;
    let source = root.join("source");
    let restored = root.join("restored");
    let output = root.join("copy.fdbk");

    let families = ColumnFamilies::open(&source, Options::default())?;
    let named = families.create("users")?;
    let numeric = families.create("123")?;
    let binary_key = b"\x00\xff\x0a".to_vec();
    let large = vec![0x73_u8; 128 * 1024];
    families.put(
        ColumnFamilyId::DEFAULT,
        &binary_key,
        &large,
        WriteOptions::sync(),
    )?;
    families.put(named, b"shared", b"alice", WriteOptions::sync())?;
    families.put(numeric, b"shared", b"number", WriteOptions::sync())?;
    families.put(named, b"removed", b"old", WriteOptions::sync())?;
    families.delete(named, b"removed", WriteOptions::sync())?;
    families.close()?;
    drop(families);

    let export = cli(&["export", as_text(&source), as_text(&output)])?;
    assert!(export.status.success(), "{}", message(&export));
    assert!(fs::metadata(&output)?.len() > 128 * 1024);
    let restore = cli(&["restore", as_text(&output), as_text(&restored)])?;
    assert!(restore.status.success(), "{}", message(&restore));

    let reopened = ColumnFamilies::open(
        &restored,
        Options {
            read_only: true,
            create_if_missing: false,
            ..Options::default()
        },
    )?;
    let names = reopened.list()?;
    assert_eq!(names.len(), 3);
    let users = names
        .iter()
        .find(|x| x.name == "users")
        .ok_or("users missing")?;
    let number = names
        .iter()
        .find(|x| x.name == "123")
        .ok_or("123 missing")?;
    assert_eq!(
        reopened.get(ColumnFamilyId::DEFAULT, &binary_key)?,
        Some(large)
    );
    assert_eq!(reopened.get(users.id, b"shared")?, Some(b"alice".to_vec()));
    assert_eq!(
        reopened.get(number.id, b"shared")?,
        Some(b"number".to_vec())
    );
    assert_eq!(reopened.get(users.id, b"removed")?, None);
    reopened.close()?;
    drop(reopened);

    let existing = cli(&["restore", as_text(&output), as_text(&restored)])?;
    assert!(!existing.status.success());
    let overwrite = cli(&["export", as_text(&source), as_text(&output)])?;
    assert!(!overwrite.status.success());
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn restore_rejects_corruption_truncation_and_future_format_without_publishing()
-> Result<(), Box<dyn std::error::Error>> {
    let root = root("corruption")?;
    let source = root.join("source");
    let archive = root.join("valid.fdbk");
    let database = Database::open(&source, Options::default())?;
    database.put(b"hello", b"world", WriteOptions::sync())?;
    database.close()?;
    drop(database);
    assert!(
        cli(&["export", as_text(&source), as_text(&archive)])?
            .status
            .success()
    );
    let valid = fs::read(&archive)?;

    let corrupted = root.join("corrupted.fdbk");
    let mut modified = valid.clone();
    let index = modified.len() - 6;
    modified[index] ^= 1;
    fs::write(&corrupted, modified)?;
    let destination = root.join("invalid-restore");
    let wrong_checksum = cli(&["restore", as_text(&corrupted), as_text(&destination)])?;
    assert!(!wrong_checksum.status.success());
    assert!(!destination.exists());

    let truncated = root.join("truncated.fdbk");
    fs::write(&truncated, &valid[..valid.len() - 3])?;
    let target = root.join("truncated-restore");
    assert!(
        !cli(&["restore", as_text(&truncated), as_text(&target)])?
            .status
            .success()
    );
    assert!(!target.exists());

    let unknown_version = root.join("future.fdbk");
    let mut modified = valid.clone();
    modified[8..12].copy_from_slice(&2_u32.to_le_bytes());
    fs::write(&unknown_version, modified)?;
    let target = root.join("future-restore");
    let result = cli(&["restore", as_text(&unknown_version), as_text(&target)])?;
    assert_eq!(result.status.code(), Some(4));
    assert!(!target.exists());

    // A failed restore must not leak partial staging directories.
    for item in fs::read_dir(&root)? {
        let name = item?.file_name().to_string_lossy().to_string();
        assert!(!name.starts_with(".forgedb-restore-"), "leaked {name}");
    }
    fs::remove_dir_all(root)?;
    Ok(())
}
