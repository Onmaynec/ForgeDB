use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_directory(name: &str) -> std::io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-cli-v070-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn invoke(path: &PathBuf, subcommand: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_forgedb"))
        .args(["cf", subcommand])
        .arg(path)
        .args(args)
        .output()
}

#[test]
fn create_list_put_get_delete_drop_are_persisted() -> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("crud")?;
    let created = invoke(&path, "create", &["sessions"])?;
    assert!(created.status.success(), "{:?}", created.stderr);
    assert_eq!(created.stdout, b"1\n");

    let list = invoke(&path, "list", &[])?;
    assert!(list.status.success());
    assert_eq!(list.stdout, b"0\tdefault\n1\tsessions\n");

    let put = invoke(&path, "put", &["1", "key", "value", "--sync"])?;
    assert!(put.status.success(), "{:?}", put.stderr);
    let got = invoke(&path, "get", &["1", "key"])?;
    assert!(got.status.success());
    assert_eq!(got.stdout, b"value\n");

    let default = invoke(&path, "get", &["0", "key"])?;
    assert!(default.status.success());
    assert!(default.stdout.is_empty());

    let deleted = invoke(&path, "delete", &["1", "key", "--sync"])?;
    assert!(deleted.status.success());
    let after = invoke(&path, "get", &["1", "key"])?;
    assert!(after.status.success());
    assert!(after.stdout.is_empty());

    assert!(invoke(&path, "drop", &["1"])?.status.success());
    let replacement = invoke(&path, "create", &["sessions"])?;
    assert_eq!(replacement.stdout, b"2\n");
    let stale = invoke(&path, "get", &["1", "key"])?;
    assert!(!stale.status.success());
    assert!(invoke(&path, "list", &[])?.status.success());

    fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn rejects_invalid_family_id_and_name() -> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("invalid")?;
    let name = invoke(&path, "create", &["../escape"])?;
    assert_eq!(name.status.code(), Some(2));
    let wrong_id = invoke(&path, "get", &["not-a-number", "key"])?;
    assert_eq!(wrong_id.status.code(), Some(2));
    fs::remove_dir_all(path)?;
    Ok(())
}
