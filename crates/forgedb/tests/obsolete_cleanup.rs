use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, ReadOptions, Result, WriteOptions};

fn directory(name: &str) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-v060-cleanup-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_cleanup_test_directory", Some(path.clone()), source))?;
    Ok(path)
}

fn options() -> Options {
    Options {
        l0_compaction_trigger: 8,
        l0_slowdown_trigger: 9,
        l0_stop_trigger: 10,
        ..Options::default()
    }
}

#[test]
fn active_scan_defers_retired_sst_until_iterator_is_dropped() -> Result<()> {
    let path = directory("iterator-lease")?;
    let database = Database::open(&path, options())?;
    database.put(b"key", b"old", WriteOptions::sync())?;
    database.flush()?;
    let iterator = database.scan(.., ReadOptions::default())?;

    database.put(b"key", b"new", WriteOptions::sync())?;
    database.flush()?;
    database.compact_range(None, None)?;

    let first = database.cleanup_obsolete_sstables()?;
    assert!(!first.deferred.is_empty());
    assert_eq!(
        iterator.collect::<Result<Vec<_>>>()?,
        vec![(b"key".to_vec(), b"old".to_vec())]
    );

    let second = database.cleanup_obsolete_sstables()?;
    assert!(!second.removed.is_empty());
    assert!(second.failed.is_empty());
    assert_eq!(database.get(b"key")?, Some(b"new".to_vec()));
    assert!(database.file_inventory()?.orphan_sst.is_empty());

    database.close()?;
    drop(database);
    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_cleanup_test_directory", Some(path), source))?;
    Ok(())
}

#[test]
fn registered_snapshot_still_reads_old_version_after_cleanup() -> Result<()> {
    let path = directory("snapshot-retention")?;
    let database = Database::open(&path, options())?;
    database.put(b"account", b"before", WriteOptions::sync())?;
    database.flush()?;
    let snapshot = database.snapshot()?;
    database.put(b"account", b"after", WriteOptions::sync())?;
    database.flush()?;
    database.compact_range(None, None)?;
    let report = database.cleanup_obsolete_sstables()?;
    assert!(!report.removed.is_empty());
    assert_eq!(
        database.get_with_options(b"account", ReadOptions::from_snapshot(&snapshot))?,
        Some(b"before".to_vec())
    );
    assert_eq!(database.get(b"account")?, Some(b"after".to_vec()));

    database.close()?;
    drop(snapshot);
    drop(database);
    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_cleanup_test_directory", Some(path), source))?;
    Ok(())
}

#[test]
fn cleanup_never_guesses_origin_of_orphan_after_restart() -> Result<()> {
    let path = directory("orphan-preserved")?;
    let database = Database::open(&path, options())?;
    database.put(b"a", b"b", WriteOptions::sync())?;
    database.flush()?;
    database.close()?;
    drop(database);

    let orphan = path.join("SST-999999.sst");
    fs::write(&orphan, b"not a live SST")
        .map_err(|source| Error::io("write_test_orphan", Some(orphan.clone()), source))?;

    let database = Database::open(&path, options())?;
    let report = database.cleanup_obsolete_sstables()?;
    assert!(report.removed.is_empty());
    assert!(report.deferred.is_empty());
    assert!(orphan.exists());
    assert!(database.file_inventory()?.orphan_sst.contains(&"SST-999999.sst".to_owned()));
    database.close()?;
    drop(database);

    let database = Database::open(
        &path,
        Options {
            read_only: true,
            ..options()
        },
    )?;
    assert!(matches!(
        database.cleanup_obsolete_sstables(),
        Err(Error::ReadOnly)
    ));
    database.close()?;
    drop(database);

    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_cleanup_test_directory", Some(path), source))?;
    Ok(())
}
