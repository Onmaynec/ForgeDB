use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, Result, WriteOptions};

fn temporary_directory(name: &str) -> Result<PathBuf> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-v050-model-{name}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
    Ok(path)
}

fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed
}

#[test]
fn generated_transaction_sequences_match_reference_model() -> Result<()> {
    let directory = temporary_directory("state-machine")?;
    let options = Options {
        l0_compaction_trigger: 8,
        l0_slowdown_trigger: 9,
        l0_stop_trigger: 10,
        ..Options::default()
    };
    let database = Database::open(&directory, options)?;
    let mut model = BTreeMap::<Vec<u8>, Vec<u8>>::new();
    let mut seed = 0x5eed_f0db_0500_u64;

    for round in 0..96_u64 {
        if round % 7 == 0 {
            let key = format!("key-{:02}", next(&mut seed) % 24).into_bytes();
            let external = format!("external-{round}").into_bytes();
            let mut transaction = database.begin_transaction()?;
            transaction.put(&key, b"stale")?;
            database.put(&key, &external, WriteOptions::sync())?;
            model.insert(key.clone(), external);
            let result = transaction.commit(WriteOptions::sync());
            assert!(matches!(result, Err(Error::Conflict { .. })));
        } else {
            let mut transaction = database.begin_transaction()?;
            let mut pending = BTreeMap::<Vec<u8>, Option<Vec<u8>>>::new();
            for operation in 0..4_u64 {
                let key = format!("key-{:02}", next(&mut seed) % 24).into_bytes();
                if (next(&mut seed) + operation) % 5 == 0 {
                    transaction.delete(&key)?;
                    pending.insert(key.clone(), None);
                    assert_eq!(transaction.get(&key)?, None);
                } else {
                    let value = format!("round-{round}-op-{operation}").into_bytes();
                    transaction.put(&key, &value)?;
                    pending.insert(key.clone(), Some(value.clone()));
                    assert_eq!(transaction.get(&key)?, Some(value));
                }
            }
            transaction.commit(WriteOptions::sync())?;
            for (key, value) in pending {
                match value {
                    Some(value) => {
                        model.insert(key, value);
                    }
                    None => {
                        model.remove(&key);
                    }
                }
            }
        }

        if round % 12 == 11 {
            database.flush()?;
        }
        for index in 0..24_u64 {
            let key = format!("key-{index:02}").into_bytes();
            assert_eq!(database.get(&key)?, model.get(&key).cloned());
        }
    }

    database.close()?;
    let reopened = Database::open(&directory, Options::default())?;
    for index in 0..24_u64 {
        let key = format!("key-{index:02}").into_bytes();
        assert_eq!(reopened.get(&key)?, model.get(&key).cloned());
    }
    reopened.close()?;
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}
