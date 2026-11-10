use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, Result, WriteBatch, WriteOptions};

fn test_dir(name: &str) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-v061-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
    Ok(path)
}

fn remove_test_dir(path: PathBuf) -> Result<()> {
    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_test_directory", Some(path), source))
}

#[test]
fn public_writes_flush_when_memtable_is_full_and_recover_after_reopen() -> Result<()> {
    let path = test_dir("auto-flush")?;
    let options = Options {
        memtable_size: 512,
        ..Options::default()
    };
    let db = Database::open(&path, options.clone())?;
    for index in 0..30_u8 {
        let key = format!("key-{index:03}");
        db.put(key.as_bytes(), vec![index; 180], WriteOptions::sync())?;
    }
    for index in 0..30_u8 {
        let key = format!("key-{index:03}");
        assert_eq!(db.get(key.as_bytes())?, Some(vec![index; 180]));
    }
    db.close()?;
    drop(db);

    let reopened = Database::open(&path, options)?;
    for index in 0..30_u8 {
        let key = format!("key-{index:03}");
        assert_eq!(reopened.get(key.as_bytes())?, Some(vec![index; 180]));
    }
    reopened.close()?;
    drop(reopened);
    remove_test_dir(path)
}

#[test]
fn oversized_batch_rejected_after_single_flush_without_partial_visibility() -> Result<()> {
    let path = test_dir("bounded-retry")?;
    let options = Options {
        memtable_size: 512,
        ..Options::default()
    };
    let limits = options.limits()?;
    let db = Database::open(&path, options)?;
    db.put(b"before", b"preserve", WriteOptions::sync())?;

    let mut batch = WriteBatch::with_limits(limits);
    batch.put(b"too-large", vec![1_u8; 1024])?;
    batch.put(b"never-applied", b"value")?;
    assert!(matches!(
        db.write(batch, WriteOptions::sync()),
        Err(Error::ResourceLimit {
            resource: "memtable_memory",
            ..
        })
    ));
    assert_eq!(db.get(b"too-large")?, None);
    assert_eq!(db.get(b"never-applied")?, None);
    assert_eq!(db.get(b"before")?, Some(b"preserve".to_vec()));
    db.put(b"after", b"success", WriteOptions::sync())?;
    db.close()?;
    drop(db);
    remove_test_dir(path)
}

#[test]
fn manual_compaction_reclaims_unpinned_retired_sst_automatically() -> Result<()> {
    let path = test_dir("auto-retire")?;
    let options = Options {
        l0_compaction_trigger: 8,
        l0_slowdown_trigger: 9,
        l0_stop_trigger: 10,
        ..Options::default()
    };
    let db = Database::open(&path, options)?;
    db.put(b"key", b"old", WriteOptions::sync())?;
    db.flush()?;
    db.put(b"key", b"new", WriteOptions::sync())?;
    db.flush()?;
    db.compact_range(None, None)?;

    assert_eq!(db.get(b"key")?, Some(b"new".to_vec()));
    assert!(db.file_inventory()?.orphan_sst.is_empty());
    let report = db.cleanup_obsolete_sstables()?;
    assert!(report.failed.is_empty());
    assert!(report.deferred.is_empty());

    db.close()?;
    drop(db);
    remove_test_dir(path)
}
