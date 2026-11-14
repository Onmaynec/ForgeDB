use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, ReadOptions, Result, WriteOptions};

fn temporary_directory(name: &str) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-v082-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_stream_test_directory", Some(path.clone()), source))?;
    Ok(path)
}

fn options() -> Options {
    Options {
        data_block_size: 1024,
        block_cache_size: 0,
        table_cache_capacity: 1,
        l0_compaction_trigger: 512,
        l0_slowdown_trigger: 513,
        l0_stop_trigger: 514,
        ..Options::default()
    }
}

fn populate(database: &Database, tables: usize, keys_per_table: usize) -> Result<()> {
    for table in 0..tables {
        for key in 0..keys_per_table {
            database.put(
                format!("key-{table:04}-{key:03}").as_bytes(),
                vec![u8::try_from(table % 251).unwrap_or_default(); 256],
                WriteOptions::no_sync(),
            )?;
        }
        database.flush()?;
    }
    Ok(())
}

#[test]
fn scan_many_sst_with_capacity_one_and_reopen_preserves_order_and_bytes() -> Result<()> {
    let directory = temporary_directory("scan-capacity-one")?;
    let database = Database::open(&directory, options())?;
    populate(&database, 48, 10)?;

    let before = database.table_handle_stats()?;
    assert_eq!(before.capacity, 1);
    assert_eq!(before.open_handles, 0);

    let mut iterator = database.scan(.., ReadOptions::default())?;
    assert_eq!(database.table_handle_stats()?.open_handles, 0);
    let mut observed = Vec::new();
    while let Some(result) = iterator.next() {
        let (key, value) = result?;
        assert_eq!(value.len(), 256);
        observed.push(key);
        assert!(database.table_handle_stats()?.open_handles <= 1);
    }
    assert_eq!(observed.len(), 480);
    assert!(observed.windows(2).all(|pair| pair[0] < pair[1]));
    let stats = database.table_handle_stats()?;
    assert!(
        stats.misses > 0,
        "later data blocks must use pooled handles"
    );
    assert!(stats.evictions > 0);
    assert!(stats.reopens > 0);
    assert_eq!(stats.open_handles, 1);
    drop(iterator);

    database.close()?;
    drop(database);
    let reopened = Database::open(&directory, options())?;
    assert_eq!(reopened.table_handle_stats()?.open_handles, 0);
    assert_eq!(
        reopened
            .scan(.., ReadOptions::default())?
            .collect::<Result<Vec<_>>>()?
            .len(),
        480
    );
    reopened.close()?;
    drop(reopened);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_stream_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn concurrent_get_and_streaming_scans_share_one_fd_budget() -> Result<()> {
    let directory = temporary_directory("concurrency")?;
    let database = Arc::new(Database::open(&directory, options())?);
    populate(&database, 12, 12)?;
    let mut threads = Vec::new();
    for worker in 0..4 {
        let database = Arc::clone(&database);
        threads.push(thread::spawn(move || -> Result<()> {
            for turn in 0..24 {
                let table = (worker + turn) % 12;
                let key = format!("key-{table:04}-005");
                assert_eq!(
                    database.get(key.as_bytes())?,
                    Some(vec![u8::try_from(table).unwrap_or_default(); 256])
                );
                assert!(database.table_handle_stats()?.open_handles <= 1);
            }
            Ok(())
        }));
    }
    let mut iterator = database.scan(.., ReadOptions::default())?;
    let mut seen = 0_usize;
    while let Some(result) = iterator.next() {
        let (key, _) = result?;
        assert!(key.starts_with(b"key-"));
        seen += 1;
        assert!(database.table_handle_stats()?.open_handles <= 1);
    }
    assert_eq!(seen, 144);
    for thread in threads {
        thread.join().map_err(|_| Error::InternalInvariant {
            invariant: "I-TEST-STREAM-WORKER",
            reason: "поток проверки SST завершился аварийно".to_owned(),
        })??;
    }
    let stats = database.table_handle_stats()?;
    assert_eq!(stats.capacity, 1);
    assert!(stats.evictions > 0);
    assert!(stats.reopens > 0);
    assert!(stats.open_handles <= 1);
    drop(iterator);

    database.close()?;
    drop(database);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_stream_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn compaction_inputs_use_bounded_pool_and_cleanup_preserves_old_scan() -> Result<()> {
    let directory = temporary_directory("compaction")?;
    let database = Database::open(&directory, options())?;
    populate(&database, 6, 12)?;
    let mut previous_scan = database.scan(.., ReadOptions::default())?;
    assert!(previous_scan.next().transpose()?.is_some());

    let before = database.table_handle_stats()?;
    database.compact_range(None, None)?;
    let after = database.table_handle_stats()?;
    assert!(
        after.misses > before.misses,
        "compaction must consume pooled input blocks"
    );
    assert!(after.open_handles <= 1);

    let first_cleanup = database.cleanup_obsolete_sstables()?;
    assert!(!first_cleanup.deferred.is_empty());
    assert_eq!(previous_scan.collect::<Result<Vec<_>>>()?.len(), 71);
    let final_cleanup = database.cleanup_obsolete_sstables()?;
    assert!(final_cleanup.failed.is_empty());
    assert!(!final_cleanup.removed.is_empty());
    assert!(database.file_inventory()?.orphan_sst.is_empty());
    assert_eq!(database.scan(.., ReadOptions::default())?.count(), 72);
    assert!(database.table_handle_stats()?.open_handles <= 1);

    database.close()?;
    drop(database);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_stream_test_directory", Some(directory), source))?;
    Ok(())
}

/// Запускать вручную: cargo test -p forgedb --test streaming_handle_pool
/// ignored_stress_thousand_sstable_scan -- --ignored
#[test]
#[ignore = "ручной stress: 1024 SST с многоблочным streaming scan"]
fn ignored_stress_thousand_sstable_scan() -> Result<()> {
    let directory = temporary_directory("thousand")?;
    let mut options = options();
    options.l0_compaction_trigger = 2048;
    options.l0_slowdown_trigger = 2049;
    options.l0_stop_trigger = 2050;
    let database = Database::open(&directory, options)?;
    populate(&database, 1024, 5)?;
    let mut seen = 0_usize;
    for result in database.scan(.., ReadOptions::default())? {
        result?;
        seen += 1;
        assert!(database.table_handle_stats()?.open_handles <= 1);
    }
    assert_eq!(seen, 5120);
    database.close()?;
    drop(database);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_stream_test_directory", Some(directory), source))?;
    Ok(())
}
