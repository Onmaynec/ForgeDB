use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, ReadOptions, Result, WriteOptions};

fn temporary_directory(name: &str) -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-v081-handles-{name}-{}-{now}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_test_dir", Some(path.clone()), source))?;
    Ok(path)
}

fn low_fd_options() -> Options {
    Options {
        block_cache_size: 0,
        table_cache_capacity: 1,
        l0_compaction_trigger: 8,
        l0_slowdown_trigger: 9,
        l0_stop_trigger: 10,
        ..Options::default()
    }
}

#[test]
fn point_read_budget_one_evicts_and_reopens_without_losing_values() -> Result<()> {
    let path = temporary_directory("budget-one")?;
    let database = Database::open(&path, low_fd_options())?;
    database.put(b"alpha", b"one", WriteOptions::sync())?;
    database.flush()?;
    database.put(b"zulu", b"two", WriteOptions::sync())?;
    database.flush()?;

    let initial = database.table_handle_stats()?;
    assert_eq!(initial.capacity, 1);
    assert_eq!(initial.open_handles, 0);

    assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec()));
    let first = database.table_handle_stats()?;
    assert_eq!(first.open_handles, 1);
    assert_eq!(first.misses, 1);
    assert_eq!(first.hits, 0);

    assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec()));
    let again = database.table_handle_stats()?;
    assert!(again.hits > first.hits);

    assert_eq!(database.get(b"zulu")?, Some(b"two".to_vec()));
    let second = database.table_handle_stats()?;
    assert_eq!(second.open_handles, 1);
    assert!(second.evictions >= 1);
    assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec()));
    let reopened = database.table_handle_stats()?;
    assert_eq!(reopened.open_handles, 1);
    assert!(reopened.reopens >= 1);
    assert!(reopened.misses >= 3);

    database.close()?;
    drop(database);
    let database = Database::open(&path, low_fd_options())?;
    assert_eq!(database.table_handle_stats()?.open_handles, 0);
    assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec()));
    assert_eq!(database.get(b"zulu")?, Some(b"two".to_vec()));
    assert!(database.table_handle_stats()?.open_handles <= 1);

    database.close()?;
    drop(database);
    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_test_dir", Some(path), source))?;
    Ok(())
}

#[test]
fn pinned_scan_survives_compaction_and_retirement_evicts_windows_handles() -> Result<()> {
    let path = temporary_directory("compaction-retirement")?;
    let database = Database::open(&path, low_fd_options())?;

    database.put(b"key", b"old", WriteOptions::sync())?;
    database.flush()?;
    assert_eq!(database.get(b"key")?, Some(b"old".to_vec()));
    // A live iterator pins the old VersionSet and underlying TableReader.
    let older_scan = database.scan(.., ReadOptions::default())?;
    database.put(b"key", b"new", WriteOptions::sync())?;
    database.flush()?;
    database.compact_range(None, None)?;

    let deferred = database.cleanup_obsolete_sstables()?;
    assert!(!deferred.deferred.is_empty());
    assert!(deferred.failed.is_empty());
    assert_eq!(
        older_scan.collect::<Result<Vec<_>>>()?,
        vec![(b"key".to_vec(), b"old".to_vec())]
    );
    // No remaining reader references: the handle LRU must close File before
    // Windows remove_file, otherwise cleanup fails with a sharing violation.
    let finished = database.cleanup_obsolete_sstables()?;
    assert!(finished.failed.is_empty());
    assert!(!finished.removed.is_empty());
    assert_eq!(database.get(b"key")?, Some(b"new".to_vec()));
    assert!(database.table_handle_stats()?.open_handles <= 1);
    assert!(database.file_inventory()?.orphan_sst.is_empty());

    database.close()?;
    drop(database);
    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_test_dir", Some(path), source))?;
    Ok(())
}

#[test]
fn transaction_conflicts_and_snapshot_reads_survive_fd_eviction() -> Result<()> {
    let path = temporary_directory("snapshot-and-conflict")?;
    let database = Database::open(&path, low_fd_options())?;
    database.put(b"account", b"before", WriteOptions::sync())?;
    database.flush()?;
    let snapshot = database.snapshot()?;
    database.put(b"other", b"value", WriteOptions::sync())?;
    database.flush()?;

    // Read both files to exercise eviction under a held MVCC snapshot.
    assert_eq!(
        database.get_with_options(b"account", ReadOptions::from_snapshot(&snapshot))?,
        Some(b"before".to_vec())
    );
    assert_eq!(database.get(b"other")?, Some(b"value".to_vec()));
    let mut transaction = database.begin_transaction()?;
    database.put(b"account", b"after", WriteOptions::sync())?;
    database.flush()?;
    assert_eq!(database.get(b"account")?, Some(b"after".to_vec()));
    assert_eq!(
        database.get_with_options(b"account", ReadOptions::from_snapshot(&snapshot))?,
        Some(b"before".to_vec())
    );
    transaction.put(b"account", b"conflict")?;
    assert!(matches!(
        transaction.commit(WriteOptions::sync()),
        Err(Error::Conflict { .. })
    ));
    assert!(database.table_handle_stats()?.open_handles <= 1);

    database.close()?;
    drop(snapshot);
    drop(database);
    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_test_dir", Some(path), source))?;
    Ok(())
}
