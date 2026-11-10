use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, ReadOptions, Result, VerifyOptions, WriteOptions};

fn temporary_directory(name: &str) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-v060-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_cache_test_dir", Some(path.clone()), source))?;
    Ok(path)
}

#[test]
fn block_cache_preserves_snapshot_versions_across_sst_blocks() -> Result<()> {
    let directory = temporary_directory("mvcc-blocks")?;
    let database = Database::open(
        &directory,
        Options {
            data_block_size: 1024,
            block_cache_size: 32 * 1024,
            ..Options::default()
        },
    )?;
    database.put(b"account", vec![1_u8; 700], WriteOptions::sync())?;
    let snapshot = database.snapshot()?;
    for marker in 2..=12_u8 {
        database.put(b"account", vec![marker; 700], WriteOptions::sync())?;
    }
    database.flush()?;

    let old = ReadOptions::from_snapshot(&snapshot);
    assert_eq!(database.get_with_options(b"account", old.clone())?, Some(vec![1; 700]));
    let after_first = database.cache_stats()?;
    assert!(after_first.misses > 0);
    assert_eq!(database.get_with_options(b"account", old)?, Some(vec![1; 700]));
    let after_second = database.cache_stats()?;
    assert!(after_second.hits > after_first.hits);
    assert_eq!(database.get(b"account")?, Some(vec![12; 700]));
    assert!(database.verify(VerifyOptions::deep())?.entries_checked >= 12);

    database.close()?;
    drop(database);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_cache_test_dir", Some(directory), source))?;
    Ok(())
}

#[test]
fn small_cache_evicts_without_changing_point_lookup_results() -> Result<()> {
    let directory = temporary_directory("evictions")?;
    let database = Database::open(
        &directory,
        Options {
            data_block_size: 1024,
            block_cache_size: 1400,
            ..Options::default()
        },
    )?;
    for key in 0..40 {
        database.put(
            format!("key-{key:04}").as_bytes(),
            vec![u8::try_from(key).unwrap_or_default(); 700],
            WriteOptions::no_sync(),
        )?;
    }
    database.flush()?;
    for key in (0..40).rev() {
        assert_eq!(
            database.get(format!("key-{key:04}").as_bytes())?,
            Some(vec![u8::try_from(key).unwrap_or_default(); 700])
        );
    }
    let stats = database.cache_stats()?;
    assert!(stats.misses > 0);
    assert!(stats.evictions > 0);
    assert!(stats.resident_bytes <= stats.capacity_bytes);

    database.close()?;
    drop(database);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_cache_test_dir", Some(directory), source))?;
    Ok(())
}

#[test]
fn cache_can_be_disabled_and_transaction_conflict_still_detected() -> Result<()> {
    let directory = temporary_directory("disabled-cache-conflict")?;
    let database = Database::open(
        &directory,
        Options {
            block_cache_size: 0,
            ..Options::default()
        },
    )?;
    database.put(b"key", b"before", WriteOptions::sync())?;
    database.flush()?;
    let mut transaction = database.begin_transaction()?;
    database.put(b"key", b"after", WriteOptions::sync())?;
    database.flush()?;
    assert_eq!(database.get(b"key")?, Some(b"after".to_vec()));
    transaction.put(b"key", b"conflict")?;
    assert!(matches!(
        transaction.commit(WriteOptions::sync()),
        Err(Error::Conflict { .. })
    ));
    let stats = database.cache_stats()?;
    assert_eq!(stats.capacity_bytes, 0);
    assert_eq!(stats.resident_blocks, 0);
    assert!(stats.bypasses > 0);

    database.close()?;
    drop(database);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_cache_test_dir", Some(directory), source))?;
    Ok(())
}
