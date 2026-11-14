use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, ReadOptions, Result, WriteBatch, WriteOptions};

fn directory(name: &str) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-v090-perf-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_perf_test_dir", Some(path.clone()), source))?;
    Ok(path)
}

fn options() -> Options {
    Options {
        data_block_size: 1024,
        block_cache_size: 0,
        table_cache_capacity: 2,
        target_sst_size: 8 * 1024,
        l0_compaction_trigger: 128,
        l0_slowdown_trigger: 129,
        l0_stop_trigger: 130,
        ..Options::default()
    }
}

#[test]
fn ordered_handle_lru_keeps_most_recent_file_and_maintains_exact_metrics() -> Result<()> {
    let dir = directory("ordered-lru")?;
    let database = Database::open(&dir, options())?;
    for (key, value) in [
        (b"alpha".as_slice(), b"one".as_slice()),
        (b"hotel".as_slice(), b"two".as_slice()),
        (b"zulu".as_slice(), b"three".as_slice()),
    ] {
        database.put(key, value, WriteOptions::sync())?;
        database.flush()?;
    }
    assert_eq!(database.table_handle_stats()?.open_handles, 0);

    assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec())); // miss
    assert_eq!(database.get(b"hotel")?, Some(b"two".to_vec())); // miss
    assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec())); // hit, alpha MRU
    let first = database.table_handle_stats()?;
    assert_eq!(first.open_handles, 2);
    assert_eq!(first.capacity, 2);
    assert_eq!(first.misses, 2);
    assert_eq!(first.hits, 1);

    assert_eq!(database.get(b"zulu")?, Some(b"three".to_vec())); // evicts hotel
    let second = database.table_handle_stats()?;
    assert_eq!(second.open_handles, 2);
    assert_eq!(second.misses, 3);
    assert_eq!(second.evictions, 1);

    assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec())); // still resident
    let third = database.table_handle_stats()?;
    assert_eq!(third.misses, second.misses);
    assert!(third.hits > second.hits);

    assert_eq!(database.get(b"hotel")?, Some(b"two".to_vec())); // reopen
    let fourth = database.table_handle_stats()?;
    assert_eq!(fourth.open_handles, 2);
    assert_eq!(fourth.misses, 4);
    assert_eq!(fourth.reopens, 1);
    assert_eq!(fourth.evictions, 2);

    database.close()?;
    drop(database);
    fs::remove_dir_all(&dir)
        .map_err(|source| Error::io("remove_perf_test_dir", Some(dir), source))?;
    Ok(())
}

#[test]
fn conflict_validation_binary_search_finds_versions_in_leveled_sst() -> Result<()> {
    let dir = directory("leveled-conflict")?;
    let database = Database::open(&dir, options())?;
    for start in (0..480).step_by(40) {
        let mut batch = WriteBatch::new();
        for index in start..start + 40 {
            batch.put(
                format!("key-{index:04}").into_bytes(),
                vec![u8::try_from(index % 251).unwrap_or_default(); 256],
            )?;
        }
        database.write(batch, WriteOptions::no_sync())?;
        database.flush()?;
    }
    // Forced compaction publishes many non-overlapping leveled SST ranges.
    database.compact_range(None, None)?;
    for index in [0, 1, 27, 239, 380, 479] {
        let key = format!("key-{index:04}");
        assert_eq!(
            database.get(key.as_bytes())?,
            Some(vec![u8::try_from(index % 251).unwrap_or_default(); 256])
        );
    }

    let mut tx = database.begin_transaction()?;
    // Updating an existing key after BEGIN makes an optimistic conflict,
    // whether its earlier version was placed in L1, L2 or another level.
    database.put(b"key-0239", b"fresh", WriteOptions::sync())?;
    database.flush()?;
    tx.put(b"key-0239", b"stale")?;
    assert!(matches!(
        tx.commit(WriteOptions::sync()),
        Err(Error::Conflict { .. })
    ));
    assert_eq!(database.get(b"key-0239")?, Some(b"fresh".to_vec()));
    assert!(database.table_handle_stats()?.open_handles <= 2);

    database.close()?;
    drop(database);
    let database = Database::open(&dir, options())?;
    assert_eq!(database.get(b"key-0239")?, Some(b"fresh".to_vec()));
    database.close()?;
    drop(database);
    fs::remove_dir_all(&dir)
        .map_err(|source| Error::io("remove_perf_test_dir", Some(dir), source))?;
    Ok(())
}

#[test]
fn mvcc_merge_discards_large_invisible_values_without_losing_snapshot_semantics() -> Result<()> {
    let dir = directory("mvcc-large-values")?;
    let database = Database::open(&dir, options())?;
    let original = vec![17_u8; 128 * 1024];
    let latest = vec![23_u8; 128 * 1024];
    database.put(b"big", &original, WriteOptions::sync())?;
    database.flush()?;
    let snapshot = database.snapshot()?;
    for marker in 0..16_u8 {
        database.put(b"big", vec![marker; 128 * 1024], WriteOptions::no_sync())?;
        database.flush()?;
    }
    database.put(b"big", &latest, WriteOptions::sync())?;
    database.flush()?;
    database.put(b"removed", b"gone", WriteOptions::sync())?;
    database.flush()?;
    database.delete(b"removed", WriteOptions::sync())?;
    database.flush()?;

    assert_eq!(
        database
            .scan(.., ReadOptions::from_snapshot(&snapshot))?
            .collect::<Result<Vec<_>>>()?,
        vec![(b"big".to_vec(), original.clone())]
    );
    assert_eq!(
        database
            .scan(.., ReadOptions::default())?
            .collect::<Result<Vec<_>>>()?,
        vec![(b"big".to_vec(), latest)]
    );
    assert_eq!(database.get(b"removed")?, None);

    database.close()?;
    drop(snapshot);
    drop(database);
    fs::remove_dir_all(&dir)
        .map_err(|source| Error::io("remove_perf_test_dir", Some(dir), source))?;
    Ok(())
}

/// Ранний range filter не должен нарушаться из-за borrowed VersionSet обхода:
/// двигаются только cursor для SST, диапазоны которых перекрывают запрос.
#[test]
fn narrow_range_scan_preserves_order_across_many_sst_without_handle_vector() -> Result<()> {
    let dir = directory("narrow-range")?;
    let database = Database::open(&dir, options())?;
    for table in 0..48 {
        let mut batch = WriteBatch::new();
        for index in 0..8 {
            batch.put(
                format!("key-{table:04}-{index:02}").into_bytes(),
                vec![u8::try_from(table).unwrap_or_default(); 32],
            )?;
        }
        database.write(batch, WriteOptions::no_sync())?;
        database.flush()?;
    }

    let entries = database
        .scan(
            b"key-0032-03".to_vec()..b"key-0033-04".to_vec(),
            ReadOptions::default(),
        )?
        .collect::<Result<Vec<_>>>()?;
    let keys = entries.into_iter().map(|(key, _)| key).collect::<Vec<_>>();
    let expected = (3..8)
        .map(|index| format!("key-0032-{index:02}").into_bytes())
        .chain((0..4).map(|index| format!("key-0033-{index:02}").into_bytes()))
        .collect::<Vec<_>>();
    assert_eq!(keys, expected);
    assert!(database.table_handle_stats()?.open_handles <= 2);

    database.close()?;
    drop(database);
    fs::remove_dir_all(&dir)
        .map_err(|source| Error::io("remove_perf_test_dir", Some(dir), source))?;
    Ok(())
}
