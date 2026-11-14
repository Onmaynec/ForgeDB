use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{
    ColumnFamilies, ColumnFamilyId, Error, Options, ReadOptions, Result, WriteBatch, WriteOptions,
};

fn temp_directory(name: &str) -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("forgedb-cf-{name}-{}-{now}", std::process::id()));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
    Ok(path)
}

fn cleanup(path: PathBuf) -> Result<()> {
    fs::remove_dir_all(&path)
        .map_err(|source| Error::io("remove_test_directory", Some(path), source))
}

#[test]
fn isolated_families_persist_same_keys_through_flush_and_reopen() -> Result<()> {
    let root = temp_directory("isolated")?;
    let families = ColumnFamilies::open(&root, Options::default())?;
    let users = families.create("users")?;
    let sessions = families.create("sessions")?;
    assert_ne!(users, sessions);
    assert_eq!(families.list()?.len(), 3);

    families.put(
        ColumnFamilyId::DEFAULT,
        b"shared",
        b"default",
        WriteOptions::sync(),
    )?;
    families.put(users, b"shared", b"user", WriteOptions::sync())?;
    families.put(sessions, b"shared", b"session", WriteOptions::sync())?;
    let mut batch = WriteBatch::new();
    batch.put(b"another", b"batch-value")?;
    batch.delete(b"shared")?;
    families.write(sessions, batch, WriteOptions::sync())?;

    families.flush(users)?;
    families.flush(sessions)?;
    families.compact_range(users, None, None)?;
    assert_eq!(
        families.get(ColumnFamilyId::DEFAULT, b"shared")?,
        Some(b"default".to_vec())
    );
    assert_eq!(families.get(users, b"shared")?, Some(b"user".to_vec()));
    assert_eq!(families.get(sessions, b"shared")?, None);
    assert_eq!(
        families.get(sessions, b"another")?,
        Some(b"batch-value".to_vec())
    );
    families.close()?;
    drop(families);

    let reopened = ColumnFamilies::open(&root, Options::default())?;
    assert_eq!(reopened.list()?.len(), 3);
    assert_eq!(
        reopened.get(ColumnFamilyId::DEFAULT, b"shared")?,
        Some(b"default".to_vec())
    );
    assert_eq!(reopened.get(users, b"shared")?, Some(b"user".to_vec()));
    assert_eq!(reopened.get(sessions, b"shared")?, None);
    assert_eq!(
        reopened.get(sessions, b"another")?,
        Some(b"batch-value".to_vec())
    );
    let rows = reopened
        .scan(sessions, .., ReadOptions::default())?
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], (b"another".to_vec(), b"batch-value".to_vec()));
    reopened.close()?;
    drop(reopened);
    cleanup(root)
}

#[test]
fn transaction_snapshots_and_conflicts_are_scoped_to_one_family() -> Result<()> {
    let root = temp_directory("transactions")?;
    let families = ColumnFamilies::open(&root, Options::default())?;
    let alpha = families.create("alpha")?;
    let beta = families.create("beta")?;

    families.put(alpha, b"same", b"a1", WriteOptions::sync())?;
    families.put(beta, b"same", b"b1", WriteOptions::sync())?;
    let mut tx = families.begin_transaction(alpha)?;
    assert_eq!(tx.get(b"same")?, Some(b"a1".to_vec()));
    families.put(beta, b"same", b"b2", WriteOptions::sync())?;
    tx.put(b"same", b"a2")?;
    tx.commit(WriteOptions::sync())?;
    assert_eq!(families.get(alpha, b"same")?, Some(b"a2".to_vec()));
    assert_eq!(families.get(beta, b"same")?, Some(b"b2".to_vec()));

    let alpha_snapshot = families.snapshot(alpha)?;
    assert!(
        families
            .get_with_options(beta, b"same", ReadOptions::from_snapshot(&alpha_snapshot),)
            .is_err()
    );
    drop(alpha_snapshot);
    families.close()?;
    drop(families);
    cleanup(root)
}

#[test]
fn drop_is_logical_and_name_reuse_cannot_recover_old_data() -> Result<()> {
    let root = temp_directory("drop")?;
    let families = ColumnFamilies::open(&root, Options::default())?;
    let previous = families.create("cache")?;
    families.put(previous, b"key", b"secret", WriteOptions::sync())?;
    families.drop_family(previous)?;
    assert!(families.get(previous, b"key").is_err());
    let replacement = families.create("cache")?;
    assert_ne!(previous, replacement);
    assert_eq!(families.get(replacement, b"key")?, None);
    assert!(families.drop_family(ColumnFamilyId::DEFAULT).is_err());
    assert!(families.create("../escape").is_err());
    assert!(root.join(".forgedb-column-families/CF-0000000001").exists());
    families.close()?;
    drop(families);

    let reopened = ColumnFamilies::open(&root, Options::default())?;
    assert_eq!(reopened.list()?.len(), 2);
    assert_eq!(reopened.get(replacement, b"key")?, None);
    assert!(reopened.get(previous, b"key").is_err());
    reopened.close()?;
    drop(reopened);
    cleanup(root)
}

#[test]
fn corrupted_registry_fails_closed_without_silent_reset() -> Result<()> {
    let root = temp_directory("checksum")?;
    let families = ColumnFamilies::open(&root, Options::default())?;
    families.create("analytics")?;
    families.close()?;
    drop(families);

    let registry = root.join(".forgedb-column-families/REGISTRY");
    let original = fs::read(&registry)
        .map_err(|source| Error::io("read_test_registry", Some(registry.clone()), source))?;
    let mut modified = original.clone();
    modified[20] ^= 0x80;
    fs::write(&registry, &modified)
        .map_err(|source| Error::io("corrupt_test_registry", Some(registry.clone()), source))?;
    assert!(matches!(
        ColumnFamilies::open(&root, Options::default()),
        Err(Error::Corruption { .. })
    ));
    fs::write(&registry, original)
        .map_err(|source| Error::io("restore_test_registry", Some(registry), source))?;

    let reopened = ColumnFamilies::open(&root, Options::default())?;
    assert_eq!(reopened.list()?.len(), 2);
    reopened.close()?;
    drop(reopened);
    cleanup(root)
}

#[test]
fn readonly_rejects_mutations_and_missing_registered_family_is_corruption() -> Result<()> {
    let root = temp_directory("readonly")?;
    let families = ColumnFamilies::open(&root, Options::default())?;
    let id = families.create("log")?;
    families.put(id, b"key", b"value", WriteOptions::sync())?;
    families.close()?;
    drop(families);

    let options = Options {
        create_if_missing: false,
        read_only: true,
        ..Options::default()
    };
    let readonly = ColumnFamilies::open(&root, options.clone())?;
    assert_eq!(readonly.get(id, b"key")?, Some(b"value".to_vec()));
    assert!(matches!(readonly.create("other"), Err(Error::ReadOnly)));
    assert!(matches!(readonly.drop_family(id), Err(Error::ReadOnly)));
    assert!(matches!(
        readonly.put(id, b"key", b"new", WriteOptions::sync()),
        Err(Error::ReadOnly)
    ));
    readonly.close()?;
    drop(readonly);

    let child = root.join(".forgedb-column-families/CF-0000000001");
    let relocated = root.join(".forgedb-column-families/missing");
    fs::rename(&child, &relocated)
        .map_err(|source| Error::io("rename_test_family", Some(child.clone()), source))?;
    let reopened = ColumnFamilies::open(&root, Options::default())?;
    assert!(matches!(
        reopened.get(id, b"key"),
        Err(Error::Corruption { .. })
    ));
    assert!(!child.exists());
    reopened.close()?;
    drop(reopened);
    cleanup(root)
}

#[test]
fn orphan_directory_from_crash_is_not_reused_or_cleaned() -> Result<()> {
    let root = temp_directory("orphan")?;
    let families = ColumnFamilies::open(&root, Options::default())?;
    let orphan = root.join(".forgedb-column-families/CF-0000000001");
    fs::create_dir_all(&orphan)
        .map_err(|source| Error::io("create_orphan", Some(orphan.clone()), source))?;
    let family = families.create("alive")?;
    assert_eq!(family.get(), 2);
    assert!(orphan.is_dir());
    assert_eq!(families.list()?.len(), 2);
    families.close()?;
    drop(families);
    cleanup(root)
}
