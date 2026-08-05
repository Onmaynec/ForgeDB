use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use forgedb_core::{DatabaseId, Error, Limits, Result, SequenceNumber, WriteBatch};
use forgedb_fs::{FileSystem, WritableFile};

use crate::codec::{WAL_FRAME_HEADER_SIZE, WAL_HEADER_SIZE, WalBatch, WalCodec, WalHeader};

pub const WAL_FILE_NUMBER: u64 = 1;
pub const WAL_FILE_NAME: &str = "WAL-000001.log";

/// Canonical имя numbered WAL.
#[must_use]
pub fn wal_file_name(file_number: u64) -> String {
    format!("WAL-{file_number:06}.log")
}

/// Разбирает canonical имя numbered WAL.
#[must_use]
pub fn parse_wal_file_name(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("WAL-")?.strip_suffix(".log")?;
    if digits.len() < 6 || digits.bytes().any(|byte| !byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

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
    file_number: u64,
    bytes_written: u64,
}

impl<W: WritableFile> WalWriter<W> {
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
        self.bytes_written = self
            .bytes_written
            .checked_add(
                u64::try_from(frame.len()).map_err(|_| Error::ResourceLimit {
                    resource: "wal_bytes_written",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?,
            )
            .ok_or(Error::ResourceLimit {
                resource: "wal_bytes_written",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        Ok(self.bytes_written)
    }

    pub fn sync(&self) -> Result<()> {
        self.file
            .sync_all()
            .map_err(|source| Error::io("sync_wal", Some(self.path.clone()), source))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn file_number(&self) -> u64 {
        self.file_number
    }
}

/// Создаёт пустой numbered WAL и синхронизирует файл и каталог.
pub fn create_wal<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    file_number: u64,
) -> Result<WalWriter<F::WritableFile>> {
    if file_number == 0 {
        return Err(Error::invalid_argument(
            "file_number",
            "ноль зарезервирован",
        ));
    }
    let path = directory.join(wal_file_name(file_number));
    let mut file = filesystem.create_new_writable(&path)?;
    file.write_all(&WalHeader::new(database_id, file_number).encode())
        .map_err(|source| Error::io("write_wal_header", Some(path.clone()), source))?;
    file.sync_all()
        .map_err(|source| Error::io("sync_wal_header", Some(path.clone()), source))?;
    drop(file);
    filesystem.sync_directory(directory)?;
    Ok(WalWriter {
        file: filesystem.open_appendable(&path)?,
        path,
        file_number,
        bytes_written: u64::try_from(WAL_HEADER_SIZE).unwrap_or(u64::MAX),
    })
}

/// Открывает или создаёт numbered WAL, предварительно выполнив replay.
pub fn open_wal_numbered<F, A>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    file_number: u64,
    limits: &Limits,
    mut apply: A,
) -> Result<(WalWriter<F::WritableFile>, ReplaySummary)>
where
    F: FileSystem,
    A: FnMut(WalBatch) -> Result<()>,
{
    let path = directory.join(wal_file_name(file_number));
    let summary = match filesystem.open_sequential(&path) {
        Ok(_) => {
            let summary = replay_wal_numbered(
                filesystem,
                &path,
                database_id,
                file_number,
                limits,
                &mut apply,
            )?;
            if summary.truncated_tail {
                filesystem.truncate_file(&path, summary.valid_bytes)?;
            }
            summary
        }
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            let writer = create_wal(filesystem, directory, database_id, file_number)?;
            let summary = ReplaySummary {
                valid_bytes: u64::try_from(WAL_HEADER_SIZE).unwrap_or(u64::MAX),
                ..ReplaySummary::default()
            };
            return Ok((writer, summary));
        }
        Err(error) => return Err(error),
    };
    Ok((
        WalWriter {
            file: filesystem.open_appendable(&path)?,
            path,
            file_number,
            bytes_written: summary.valid_bytes,
        },
        summary,
    ))
}

/// Совместимый wrapper для WAL №1.
pub fn open_wal<F, A>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    limits: &Limits,
    apply: A,
) -> Result<(WalWriter<F::WritableFile>, ReplaySummary)>
where
    F: FileSystem,
    A: FnMut(WalBatch) -> Result<()>,
{
    open_wal_numbered(
        filesystem,
        directory,
        database_id,
        WAL_FILE_NUMBER,
        limits,
        apply,
    )
}

/// Строго воспроизводит numbered WAL.
pub fn replay_wal_numbered<F, A>(
    filesystem: &F,
    path: &Path,
    database_id: DatabaseId,
    file_number: u64,
    limits: &Limits,
    mut apply: A,
) -> Result<ReplaySummary>
where
    F: FileSystem,
    A: FnMut(WalBatch) -> Result<()>,
{
    limits.validate()?;
    let mut reader = filesystem.open_sequential(path)?;
    let mut header = [0_u8; WAL_HEADER_SIZE];
    if read_up_to(&mut reader, &mut header)
        .map_err(|source| Error::io("read_wal_header", Some(path.into()), source))?
        != WAL_HEADER_SIZE
    {
        return Err(Error::corruption(
            "WAL header",
            "файл усечён до конца заголовка",
        ));
    }
    WalHeader::decode(&header, database_id, file_number)?;
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
        if read_up_to(&mut reader, &mut payload)
            .map_err(|source| Error::io("read_wal_payload", Some(path.into()), source))?
            != descriptor.payload_length
        {
            summary.truncated_tail = true;
            return Ok(summary);
        }
        let decoded = WalCodec::decode_complete_frame(&frame_header, &payload, limits)?;
        if decoded.sequence_start < expected_sequence {
            return Err(Error::corruption(
                "WAL sequence",
                format!(
                    "ожидалась sequence не меньше {}, найдена {}",
                    expected_sequence.get(),
                    decoded.sequence_start.get()
                ),
            ));
        }
        let end = decoded.sequence_start.inclusive_end(decoded.batch.len())?;
        apply(decoded)?;
        summary.frame_count = summary
            .frame_count
            .checked_add(1)
            .ok_or(Error::ResourceLimit {
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

/// Совместимый wrapper, извлекающий номер из canonical имени или использующий №1.
pub fn replay_wal<F, A>(
    filesystem: &F,
    path: &Path,
    database_id: DatabaseId,
    limits: &Limits,
    apply: A,
) -> Result<ReplaySummary>
where
    F: FileSystem,
    A: FnMut(WalBatch) -> Result<()>,
{
    let number = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(parse_wal_file_name)
        .unwrap_or(WAL_FILE_NUMBER);
    replay_wal_numbered(filesystem, path, database_id, number, limits, apply)
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
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use forgedb_fs::NativeFileSystem;

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-wal-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn numbered_wal_round_trip() -> Result<()> {
        let directory = temporary_directory("numbered")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let mut writer = create_wal(&filesystem, &directory, database_id, 7)?;
        let mut batch = WriteBatch::new();
        batch.put(b"key", b"value")?;
        writer.append_batch(SequenceNumber::FIRST, &batch, &limits, true)?;
        drop(writer);
        let mut replayed = 0_usize;
        let summary = replay_wal_numbered(
            &filesystem,
            &directory.join(wal_file_name(7)),
            database_id,
            7,
            &limits,
            |_| {
                replayed += 1;
                Ok(())
            },
        )?;
        assert_eq!(summary.frame_count, 1);
        assert_eq!(replayed, 1);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
