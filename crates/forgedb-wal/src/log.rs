use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use forgedb_core::{DatabaseId, Error, Limits, Result, SequenceNumber, WriteBatch};
use forgedb_fs::{FileSystem, WritableFile};

use crate::codec::{
    WAL_FRAME_HEADER_SIZE, WAL_HEADER_SIZE, WalBatch, WalCodec, WalHeader,
};

pub const WAL_FILE_NUMBER: u64 = 1;
pub const WAL_FILE_NAME: &str = "WAL-000001.log";

/// Итог строгого последовательного replay одного WAL.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReplaySummary {
    pub frame_count: u64,
    pub operation_count: u64,
    pub last_sequence: Option<SequenceNumber>,
    pub valid_bytes: u64,
    pub truncated_tail: bool,
}

/// Append-only writer для одного WAL-файла.
#[derive(Debug)]
pub struct WalWriter<W: WritableFile> {
    file: W,
    path: PathBuf,
    bytes_written: u64,
}

impl<W: WritableFile> WalWriter<W> {
    /// Добавляет один полностью сформированный кадр и при необходимости синхронизирует файл.
    pub fn append_batch(
        &mut self,
        sequence_start: SequenceNumber,
        batch: &WriteBatch,
        limits: &Limits,
        sync: bool,
    ) -> Result<u64> {
        let frame = WalCodec::encode_frame(sequence_start, batch, limits)?;
        self.file
            .write_all(&frame)
            .map_err(|source| Error::io("append_wal_frame", Some(self.path.clone()), source))?;
        if sync {
            self.file
                .sync_all()
                .map_err(|source| Error::io("sync_wal", Some(self.path.clone()), source))?;
        }
        let frame_len = u64::try_from(frame.len()).map_err(|_| Error::ResourceLimit {
            resource: "wal_bytes_written",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        self.bytes_written = self
            .bytes_written
            .checked_add(frame_len)
            .ok_or(Error::ResourceLimit {
                resource: "wal_bytes_written",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        Ok(self.bytes_written)
    }

    /// Синхронизирует ранее записанные байты WAL.
    pub fn sync(&self) -> Result<()> {
        self.file
            .sync_all()
            .map_err(|source| Error::io("sync_wal", Some(self.path.clone()), source))
    }

    /// Возвращает физический путь WAL.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Открывает существующий WAL или создаёт новый, предварительно выполнив replay.
pub fn open_wal<F, A>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    limits: &Limits,
    mut apply: A,
) -> Result<(WalWriter<F::WritableFile>, ReplaySummary)>
where
    F: FileSystem,
    A: FnMut(WalBatch) -> Result<()>,
{
    let path = directory.join(WAL_FILE_NAME);
    let summary = match filesystem.open_sequential(&path) {
        Ok(_) => {
            let summary = replay_wal(filesystem, &path, database_id, limits, &mut apply)?;
            if summary.truncated_tail {
                filesystem.truncate_file(&path, summary.valid_bytes)?;
            }
            summary
        }
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            let header = WalHeader::new(database_id, WAL_FILE_NUMBER).encode();
            let mut file = filesystem.create_new_writable(&path)?;
            file.write_all(&header)
                .map_err(|source| Error::io("write_wal_header", Some(path.clone()), source))?;
            file.sync_all()
                .map_err(|source| Error::io("sync_wal_header", Some(path.clone()), source))?;
            drop(file);
            filesystem.sync_directory(directory)?;
            ReplaySummary {
                valid_bytes: u64::try_from(WAL_HEADER_SIZE).unwrap_or(u64::MAX),
                ..ReplaySummary::default()
            }
        }
        Err(error) => return Err(error),
    };
    let file = filesystem.open_appendable(&path)?;
    Ok((
        WalWriter {
            file,
            path,
            bytes_written: summary.valid_bytes,
        },
        summary,
    ))
}

/// Строго воспроизводит все полные кадры WAL и допускает только последний физически усечённый кадр.
pub fn replay_wal<F, A>(
    filesystem: &F,
    path: &Path,
    database_id: DatabaseId,
    limits: &Limits,
    mut apply: A,
) -> Result<ReplaySummary>
where
    F: FileSystem,
    A: FnMut(WalBatch) -> Result<()>,
{
    limits.validate()?;
    let mut reader = filesystem.open_sequential(path)?;
    let mut header_bytes = [0_u8; WAL_HEADER_SIZE];
    let header_read = read_up_to(&mut reader, &mut header_bytes)
        .map_err(|source| Error::io("read_wal_header", Some(path.into()), source))?;
    if header_read != WAL_HEADER_SIZE {
        return Err(Error::corruption("WAL header", "файл усечён до конца заголовка"));
    }
    WalHeader::decode(&header_bytes, database_id, WAL_FILE_NUMBER)?;

    let mut summary = ReplaySummary {
        valid_bytes: u64::try_from(WAL_HEADER_SIZE).unwrap_or(u64::MAX),
        ..ReplaySummary::default()
    };
    let mut expected_sequence = SequenceNumber::FIRST;

    loop {
        let mut frame_header = [0_u8; WAL_FRAME_HEADER_SIZE];
        let read = read_up_to(&mut reader, &mut frame_header)
            .map_err(|source| Error::io("read_wal_frame_header", Some(path.into()), source))?;
        if read == 0 {
            return Ok(summary);
        }
        if read != WAL_FRAME_HEADER_SIZE {
            summary.truncated_tail = true;
            return Ok(summary);
        }
        let descriptor = WalCodec::decode_frame_header(&frame_header, limits)?;
        let mut payload = vec![0_u8; descriptor.payload_length];
        let payload_read = read_up_to(&mut reader, &mut payload)
            .map_err(|source| Error::io("read_wal_payload", Some(path.into()), source))?;
        if payload_read != descriptor.payload_length {
            summary.truncated_tail = true;
            return Ok(summary);
        }
        let decoded = WalCodec::decode_complete_frame(&frame_header, &payload, limits)?;
        if decoded.sequence_start != expected_sequence {
            return Err(Error::corruption(
                "WAL sequence",
                format!(
                    "ожидалась sequence {}, найдена {}",
                    expected_sequence.get(),
                    decoded.sequence_start.get()
                ),
            ));
        }
        let end = decoded.sequence_start.inclusive_end(decoded.batch.len())?;
        apply(decoded)?;

        summary.frame_count = summary.frame_count.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "wal_frame_count",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        summary.operation_count = summary
            .operation_count
            .checked_add(u64::try_from(descriptor.operation_count).map_err(|_| {
                Error::ResourceLimit {
                    resource: "wal_operation_count",
                    limit: u64::MAX,
                    observed: u64::MAX,
                }
            })?)
            .ok_or(Error::ResourceLimit {
                resource: "wal_operation_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        summary.last_sequence = Some(end);
        summary.valid_bytes = summary
            .valid_bytes
            .checked_add(u64::try_from(descriptor.total_length).map_err(|_| {
                Error::ResourceLimit {
                    resource: "wal_valid_bytes",
                    limit: u64::MAX,
                    observed: u64::MAX,
                }
            })?)
            .ok_or(Error::ResourceLimit {
                resource: "wal_valid_bytes",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        expected_sequence = end.checked_next()?;
    }
}

fn read_up_to<R: Read>(reader: &mut R, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0_usize;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_fs::NativeFileSystem;

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("forgedb-wal-{name}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn trailing_partial_frame_is_ignored_and_truncated_on_open() -> Result<()> {
        let directory = temporary_directory("partial")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let (mut writer, _) = open_wal(&filesystem, &directory, database_id, &limits, |_| Ok(()))?;
        let mut batch = WriteBatch::new();
        batch.put(b"key", b"value")?;
        let valid_bytes = writer.append_batch(SequenceNumber::FIRST, &batch, &limits, true)?;
        drop(writer);

        let path = directory.join(WAL_FILE_NAME);
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|source| Error::io("open_test_wal", Some(path.clone()), source))?;
        file.write_all(b"FWF1\x20")
            .map_err(|source| Error::io("append_partial_test_frame", Some(path.clone()), source))?;
        drop(file);

        let mut replayed = 0_usize;
        let (writer, summary) = open_wal(&filesystem, &directory, database_id, &limits, |_| {
            replayed += 1;
            Ok(())
        })?;
        assert!(summary.truncated_tail);
        assert_eq!(summary.valid_bytes, valid_bytes);
        assert_eq!(replayed, 1);
        drop(writer);
        assert_eq!(filesystem.file_size(&path)?, valid_bytes);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn checksum_mismatch_in_full_frame_blocks_replay() -> Result<()> {
        let directory = temporary_directory("corruption")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let (mut writer, _) = open_wal(&filesystem, &directory, database_id, &limits, |_| Ok(()))?;
        let mut first = WriteBatch::new();
        first.put(b"a", b"one")?;
        writer.append_batch(SequenceNumber::FIRST, &first, &limits, true)?;
        let mut second = WriteBatch::new();
        second.put(b"b", b"two")?;
        writer.append_batch(SequenceNumber::new(2)?, &second, &limits, true)?;
        drop(writer);

        let path = directory.join(WAL_FILE_NAME);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| Error::io("open_test_wal", Some(path.clone()), source))?;
        let corrupt_offset = u64::try_from(WAL_HEADER_SIZE + WAL_FRAME_HEADER_SIZE)
            .unwrap_or(u64::MAX);
        file.seek(SeekFrom::Start(corrupt_offset))
            .map_err(|source| Error::io("seek_test_wal", Some(path.clone()), source))?;
        file.write_all(&[0xff])
            .map_err(|source| Error::io("corrupt_test_wal", Some(path.clone()), source))?;
        file.sync_all()
            .map_err(|source| Error::io("sync_test_wal", Some(path.clone()), source))?;
        drop(file);

        assert!(matches!(
            replay_wal(&filesystem, &path, database_id, &limits, |_| Ok(())),
            Err(Error::Corruption { .. })
        ));
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
