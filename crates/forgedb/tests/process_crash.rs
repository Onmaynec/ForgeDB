use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, Result, WriteOptions};

const CHILD_MARKER: &str = "FORGEDB_CRASH_CHILD";
const DIRECTORY_VARIABLE: &str = "FORGEDB_CRASH_DIRECTORY";

#[test]
fn sync_write_survives_process_abort() -> Result<()> {
    if std::env::var_os(CHILD_MARKER).is_some() {
        let directory = std::env::var_os(DIRECTORY_VARIABLE)
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error::invalid_argument(
                    DIRECTORY_VARIABLE,
                    "дочернему процессу не передан каталог тестовой базы",
                )
            })?;
        let database = Database::open(&directory, Options::default())?;
        database.put(b"committed", b"survives", WriteOptions::sync())?;
        std::process::abort();
    }

    let directory = temporary_directory("process-abort")?;
    let executable = std::env::current_exe()
        .map_err(|source| Error::io("current_test_executable", None, source))?;
    let status = Command::new(executable)
        .arg("--exact")
        .arg("sync_write_survives_process_abort")
        .arg("--nocapture")
        .env(CHILD_MARKER, "1")
        .env(DIRECTORY_VARIABLE, &directory)
        .status()
        .map_err(|source| Error::io("spawn_crash_child", None, source))?;
    assert!(!status.success());

    let database = Database::open(&directory, Options::default())?;
    assert_eq!(database.get(b"committed")?, Some(b"survives".to_vec()));
    database.close()?;
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

fn temporary_directory(name: &str) -> Result<PathBuf> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("forgedb-{name}-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
    Ok(path)
}
