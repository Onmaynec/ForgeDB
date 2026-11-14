use std::fs;
use std::ops::Bound;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, ReadOptions, Result, WriteBatch, WriteOptions};

fn temp() -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "forgedb-ga-range-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&directory)
        .map_err(|source| Error::io("create_stable_test_dir", Some(directory.clone()), source))?;
    Ok(directory)
}

fn key(index: usize) -> Vec<u8> {
    format!("key-{index:04}").into_bytes()
}

fn keys(
    db: &Database,
    start: Bound<Vec<u8>>,
    end: Bound<Vec<u8>>,
    options: ReadOptions,
) -> Result<Vec<Vec<u8>>> {
    db.scan((start, end), options)?
        .map(|row| row.map(|(key, _)| key))
        .collect()
}

#[test]
fn leveled_binary_range_seek_respects_included_excluded_unbounded_and_snapshot()
-> Result<()> {
    let path = temp()?;
    let opts = Options {
        target_sst_size: 8 * 1024,
        data_block_size: 1024,
        l0_compaction_trigger: 256,
        l0_slowdown_trigger: 257,
        l0_stop_trigger: 258,
        ..Options::default()
    };
    let db = Database::open(&path, opts.clone())?;
    for start in (0..640).step_by(32) {
        let mut batch = WriteBatch::new();
        for index in start..start + 32 {
            batch.put(key(index), vec![u8::try_from(index % 251).unwrap_or(0); 256])?;
        }
        db.write(batch, WriteOptions::no_sync())?;
        db.flush()?;
    }
    db.compact_range(None, None)?;
    // Every test range now plans only the SST files intersecting its bounds.
    assert!(db.file_inventory()?.live_sst.len() > 1);
    let old = db.snapshot()?;
    db.delete(key(205), WriteOptions::sync())?;
    db.flush()?;

    assert_eq!(
        keys(&db, Bound::Included(key(200)), Bound::Excluded(key(210)), ReadOptions::default())?,
        (200..210).filter(|i| *i != 205).map(key).collect::<Vec<_>>()
    );
    assert_eq!(
        keys(&db, Bound::Included(key(200)), Bound::Excluded(key(210)), ReadOptions::from_snapshot(&old))?,
        (200..210).map(key).collect::<Vec<_>>()
    );
    assert_eq!(
        keys(&db, Bound::Excluded(key(200)), Bound::Included(key(210)), ReadOptions::default())?,
        (201..=210).filter(|i| *i != 205).map(key).collect::<Vec<_>>()
    );
    assert_eq!(
        keys(&db, Bound::Excluded(key(200)), Bound::Excluded(key(200)), ReadOptions::default())?,
        Vec::<Vec<u8>>::new()
    );
    assert_eq!(
        keys(&db, Bound::Unbounded, Bound::Excluded(key(3)), ReadOptions::default())?,
        (0..3).map(key).collect::<Vec<_>>()
    );
    assert_eq!(
        keys(&db, Bound::Included(key(638)), Bound::Unbounded, ReadOptions::default())?,
        vec![key(638), key(639)]
    );
    assert_eq!(
        keys(&db, Bound::Included(key(1000)), Bound::Unbounded, ReadOptions::default())?,
        Vec::<Vec<u8>>::new()
    );

    db.close()?;
    drop(old);
    drop(db);
    let reopened = Database::open(&path, opts)?;
    assert_eq!(
        keys(&reopened, Bound::Included(key(638)), Bound::Unbounded, ReadOptions::default())?,
        vec![key(638), key(639)]
    );
    reopened.close()?;
    drop(reopened);
    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_stable_test_dir", Some(path), source))?;
    Ok(())
}
