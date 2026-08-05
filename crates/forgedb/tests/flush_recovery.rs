use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Limits, Options, Result, SequenceNumber, WriteBatch, WriteOptions};
use forgedb_fs::{NativeFileSystem, load_or_create_identity};
use forgedb_manifest::CURRENT_FILE_NAME;
use forgedb_wal::{create_wal, wal_file_name};

const SST_HEADER_CHECKSUM_OFFSET: u64 = 60;

fn temporary_directory(name: &str) -> Result<PathBuf> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-flush-integration-{name}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&path)
        .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
    Ok(path)
}

#[test]
fn installed_sstable_survives_reopen_without_obsolete_wal() -> Result<()> {
    let directory = temporary_directory("reopen")?;
    let database = Database::open(&directory, Options::default())?;
    database.put(b"alpha", b"one", WriteOptions::sync())?;
    assert!(directory.join("WAL-000001.log").exists());

    database.flush()?;
    assert!(directory.join("SST-000002.sst").exists());
    assert!(directory.join("WAL-000003.log").exists());
    assert!(!directory.join("WAL-000001.log").exists());
    database.close()?;

    let reopened = Database::open(&directory, Options::default())?;
    assert_eq!(reopened.get(b"alpha")?, Some(b"one".to_vec()));
    reopened.close()?;
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn installed_sstable_checksum_corruption_blocks_open() -> Result<()> {
    let directory = temporary_directory("corruption")?;
    let database = Database::open(&directory, Options::default())?;
    database.put(b"alpha", b"one", WriteOptions::sync())?;
    database.flush()?;
    database.close()?;

    let table_path = directory.join("SST-000002.sst");
    let mut table = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&table_path)
        .map_err(|source| Error::io("open_test_sstable", Some(table_path.clone()), source))?;
    table
        .seek(SeekFrom::Start(SST_HEADER_CHECKSUM_OFFSET))
        .map_err(|source| Error::io("seek_test_sstable", Some(table_path.clone()), source))?;
    let mut checksum_byte = [0_u8; 1];
    table
        .read_exact(&mut checksum_byte)
        .map_err(|source| Error::io("read_test_sstable_checksum", Some(table_path.clone()), source))?;
    checksum_byte[0] ^= 0xff;
    table
        .seek(SeekFrom::Start(SST_HEADER_CHECKSUM_OFFSET))
        .map_err(|source| Error::io("seek_test_sstable", Some(table_path.clone()), source))?;
    table
        .write_all(&checksum_byte)
        .map_err(|source| Error::io("corrupt_test_sstable", Some(table_path.clone()), source))?;
    table
        .sync_all()
        .map_err(|source| Error::io("sync_test_sstable", Some(table_path), source))?;
    drop(table);

    assert!(matches!(
        Database::open(&directory, Options::default()),
        Err(Error::Corruption { .. })
    ));
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn orphan_sstable_does_not_override_manifest_state() -> Result<()> {
    let directory = temporary_directory("orphan")?;
    let database = Database::open(&directory, Options::default())?;
    database.put(b"key", b"live", WriteOptions::sync())?;
    database.flush()?;
    database.close()?;

    fs::copy(
        directory.join("SST-000002.sst"),
        directory.join("SST-999999.sst"),
    )
    .map_err(|source| Error::io("copy_orphan_sstable", Some(directory.clone()), source))?;

    let reopened = Database::open(&directory, Options::default())?;
    assert_eq!(reopened.get(b"key")?, Some(b"live".to_vec()));
    reopened.close()?;
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn missing_manifest_owned_wal_blocks_open() -> Result<()> {
    let directory = temporary_directory("missing-active-wal")?;
    let database = Database::open(&directory, Options::default())?;
    database.put(b"durable", b"value", WriteOptions::sync())?;
    database.close()?;

    let wal_path = directory.join("WAL-000001.log");
    fs::remove_file(&wal_path)
        .map_err(|source| Error::io("remove_test_wal", Some(wal_path), source))?;

    assert!(matches!(
        Database::open(&directory, Options::default()),
        Err(Error::Corruption { .. })
    ));
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn database_bootstrap_recovers_header_only_wal_before_current() -> Result<()> {
    let directory = temporary_directory("bootstrap-retry")?;
    let filesystem = NativeFileSystem;
    let identity = load_or_create_identity(&filesystem, &directory)?;
    drop(create_wal(&filesystem, &directory, identity, 1)?);
    assert!(!directory.join(CURRENT_FILE_NAME).exists());

    let database = Database::open(&directory, Options::default())?;
    database.put(b"key", b"value", WriteOptions::sync())?;
    database.close()?;
    assert!(directory.join(CURRENT_FILE_NAME).exists());
    assert!(directory.join(wal_file_name(1)).exists());

    let reopened = Database::open(&directory, Options::default())?;
    assert_eq!(reopened.get(b"key")?, Some(b"value".to_vec()));
    reopened.close()?;
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn flush_reuses_header_only_orphan_next_wal() -> Result<()> {
    let directory = temporary_directory("orphan-next-wal")?;
    let database = Database::open(&directory, Options::default())?;
    let identity = database.identity();
    database.put(b"key", b"value", WriteOptions::sync())?;
    database.close()?;

    let filesystem = NativeFileSystem;
    drop(create_wal(&filesystem, &directory, identity, 3)?);
    let reopened = Database::open(&directory, Options::default())?;
    reopened.flush()?;
    assert!(directory.join("SST-000002.sst").exists());
    assert!(directory.join(wal_file_name(3)).exists());
    assert!(!directory.join(wal_file_name(1)).exists());
    reopened.close()?;

    let verified = Database::open(&directory, Options::default())?;
    assert_eq!(verified.get(b"key")?, Some(b"value".to_vec()));
    verified.close()?;
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}

#[test]
fn flush_refuses_nonempty_orphan_next_wal() -> Result<()> {
    let directory = temporary_directory("nonempty-orphan-wal")?;
    let database = Database::open(&directory, Options::default())?;
    let identity = database.identity();
    database.put(b"live", b"value", WriteOptions::sync())?;
    database.close()?;

    let filesystem = NativeFileSystem;
    let mut orphan = create_wal(&filesystem, &directory, identity, 3)?;
    let limits = Limits::default();
    let mut batch = WriteBatch::new();
    batch.put(b"foreign", b"must-not-be-deleted")?;
    orphan.append_batch(SequenceNumber::new(2)?, &batch, &limits, true)?;
    drop(orphan);

    let reopened = Database::open(&directory, Options::default())?;
    assert!(matches!(reopened.flush(), Err(Error::Corruption { .. })));
    assert_eq!(reopened.get(b"live")?, Some(b"value".to_vec()));
    assert!(directory.join(wal_file_name(3)).exists());
    reopened.close()?;
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
    Ok(())
}
