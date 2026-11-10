use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_directory(name: &str) -> std::io::Result<PathBuf> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-cli-v050-{name}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&path)?;
    Ok(path)
}

#[test]
fn transaction_command_commits_all_mutations() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("commit")?;
    let binary = env!("CARGO_BIN_EXE_forgedb");

    let status = Command::new(binary)
        .arg("transaction")
        .arg(&directory)
        .args(["--put", "a", "one", "--put", "b", "two", "--sync"])
        .status()?;
    assert!(status.success());

    let first = Command::new(binary)
        .arg("get")
        .arg(&directory)
        .arg("a")
        .output()?;
    let second = Command::new(binary)
        .arg("get")
        .arg(&directory)
        .arg("b")
        .output()?;
    assert!(first.status.success());
    assert!(second.status.success());
    assert_eq!(first.stdout, b"one\n");
    assert_eq!(second.stdout, b"two\n");

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn transaction_command_rollback_publishes_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("rollback")?;
    let binary = env!("CARGO_BIN_EXE_forgedb");

    let status = Command::new(binary)
        .arg("tx")
        .arg(&directory)
        .args(["--put", "key", "value", "--rollback"])
        .status()?;
    assert!(status.success());

    let output = Command::new(binary)
        .arg("get")
        .arg(&directory)
        .arg("key")
        .output()?;
    assert!(output.status.success());
    assert!(output.stdout.is_empty());

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn transaction_command_rejects_disable_wal() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("disable-wal")?;
    let binary = env!("CARGO_BIN_EXE_forgedb");

    let status = Command::new(binary)
        .arg("tx")
        .arg(&directory)
        .args(["--put", "key", "value", "--disable-wal"])
        .status()?;
    assert_eq!(status.code(), Some(2));

    fs::remove_dir_all(directory)?;
    Ok(())
}
