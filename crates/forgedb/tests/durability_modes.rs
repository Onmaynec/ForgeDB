use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, Result, WriteOptions};

#[test]
fn disable_wal_gap_does_not_break_later_replay() -> Result<()> {
    let directory = temporary_directory("disable-wal-gap")?;
    let database = Database::open(&directory, Options::default())?;
    database.put(b"ephemeral", b"lost", WriteOptions::disable_wal())?;
    database.put(b"durable", b"kept", WriteOptions::sync())?;
    database.close()?;

    let reopened = Database::open(&directory, Options::default())?;
    assert_eq!(reopened.get(b"ephemeral")?, None);
    assert_eq!(reopened.get(b"durable")?, Some(b"kept".to_vec()));
    reopened.close()?;
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

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
