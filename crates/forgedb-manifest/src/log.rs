use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb_core::{DatabaseId, Error, Limits, Result};
use forgedb_fs::{FileSystem, WritableFile};

use crate::checksum::crc32c;
use crate::codec::{decode_complete_frame, decode_frame_header, encode_edit_frame};
use crate::state::{ManifestState, VersionEdit};
use crate::{
    CURRENT_FILE_NAME, INITIAL_LOG_NUMBER, MANIFEST_FORMAT_VERSION, MANIFEST_FRAME_HEADER_SIZE,
    MANIFEST_HEADER_SIZE, MINIMUM_READER_VERSION, manifest_file_name, parse_manifest_file_name,
};

const MANIFEST_MAGIC: &[u8; 8] = b"FORGEMAN";

/// Итог строгого replay MANIFEST.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReplaySummary {
    pub edit_count: u64,
    pub valid_bytes: u64,
    pub truncated_tail: bool,
}

/// Append-only writer активного MANIFEST.
#[derive(Debug)]
pub struct ManifestWriter<W: WritableFile> {
    file: W,
    path: PathBuf,
    bytes_written: u64,
}

impl<W: WritableFile> ManifestWriter<W> {
    /// Preflight-применяет, записывает, синхронизирует и затем публикует edit.
    pub fn append_edit(
        &mut self,
        state: &mut ManifestState,
        edit: &VersionEdit,
        limits: &Limits,
    ) -> Result<()> {
        let next = state.applied(edit, limits)?;
        let frame = encode_edit_frame(edit, limits)?;
        self.file
            .write_all(&frame)
            .map_err(|source| Error::io("append_manifest_edit", Some(self.path.clone()), source))?;
        self.file
            .sync_all()
            .map_err(|source| Error::io("sync_manifest", Some(self.path.clone()), source))?;
        self.bytes_written = self
            .bytes_written
            .checked_add(
                u64::try_from(frame.len()).map_err(|_| Error::ResourceLimit {
                    resource: "manifest_bytes_written",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?,
            )
            .ok_or(Error::ResourceLimit {
                resource: "manifest_bytes_written",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        *state = next;
        Ok(())
    }

    pub fn sync(&self) -> Result<()> {
        self.file
            .sync_all()
            .map_err(|source| Error::io("sync_manifest", Some(self.path.clone()), source))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Создаёт initial MANIFEST и crash-safe CURRENT.
pub fn create_manifest<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    initial_log_number: u64,
    limits: &Limits,
) -> Result<(ManifestWriter<F::WritableFile>, ManifestState)> {
    limits.validate()?;
    let state = ManifestState::initial(database_id, initial_log_number)?;
    let name = manifest_file_name(state.manifest_number);
    let temporary = directory.join(format!("{name}.tmp"));
    let final_path = directory.join(&name);
    let snapshot = encode_edit_frame(&state.snapshot_edit(), limits)?;
    let mut file = filesystem.create_writable(&temporary)?;
    file.write_all(&encode_header(database_id, state.manifest_number))
        .and_then(|_| file.write_all(&snapshot))
        .map_err(|source| Error::io("write_manifest", Some(temporary.clone()), source))?;
    file.sync_all()
        .map_err(|source| Error::io("sync_manifest", Some(temporary.clone()), source))?;
    drop(file);
    filesystem.rename_atomic(&temporary, &final_path)?;
    filesystem.sync_directory(directory)?;
    write_current(filesystem, directory, &name)?;
    let bytes_written =
        u64::try_from(MANIFEST_HEADER_SIZE + snapshot.len()).map_err(|_| Error::ResourceLimit {
            resource: "manifest_bytes_written",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    Ok((
        ManifestWriter {
            file: filesystem.open_appendable(&final_path)?,
            path: final_path,
            bytes_written,
        },
        state,
    ))
}

/// Результат открытия active MANIFEST.
#[derive(Debug)]
pub struct OpenManifest<W: WritableFile> {
    /// Writer для read-write открытия; отсутствует в read-only режиме.
    pub writer: Option<ManifestWriter<W>>,
    /// Восстановленное authoritative состояние.
    pub state: ManifestState,
    /// Итог строгого replay.
    pub replay: ReplaySummary,
}

/// Открывает active MANIFEST из CURRENT.
pub fn open_manifest<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    limits: &Limits,
    read_only: bool,
) -> Result<OpenManifest<F::WritableFile>> {
    limits.validate()?;
    let name = read_current(filesystem, directory)?;
    let number = parse_manifest_file_name(&name)
        .ok_or_else(|| Error::corruption("CURRENT", "имя MANIFEST не canonical"))?;
    let path = directory.join(name);
    let (state, summary) = replay_manifest(filesystem, &path, database_id, number, limits)?;
    if summary.truncated_tail && !read_only {
        filesystem.truncate_file(&path, summary.valid_bytes)?;
    }
    let writer = if read_only {
        None
    } else {
        Some(ManifestWriter {
            file: filesystem.open_appendable(&path)?,
            path,
            bytes_written: summary.valid_bytes,
        })
    };
    Ok(OpenManifest {
        writer,
        state,
        replay: summary,
    })
}

/// Строго replayed complete edits; только последний физически неполный frame допустим.
pub fn replay_manifest<F: FileSystem>(
    filesystem: &F,
    path: &Path,
    database_id: DatabaseId,
    manifest_number: u64,
    limits: &Limits,
) -> Result<(ManifestState, ReplaySummary)> {
    let mut reader = filesystem.open_sequential(path)?;
    let mut header = [0_u8; MANIFEST_HEADER_SIZE];
    if read_up_to(&mut reader, &mut header)
        .map_err(|source| Error::io("read_manifest_header", Some(path.into()), source))?
        != MANIFEST_HEADER_SIZE
    {
        return Err(Error::corruption(
            "MANIFEST header",
            "файл усечён до конца заголовка",
        ));
    }
    decode_header(&header, database_id, manifest_number)?;

    let mut state = ManifestState::initial(database_id, INITIAL_LOG_NUMBER)?;
    state.manifest_number = manifest_number;
    let mut initialized = false;
    let mut summary = ReplaySummary {
        valid_bytes: u64::try_from(MANIFEST_HEADER_SIZE).unwrap_or(u64::MAX),
        ..ReplaySummary::default()
    };
    loop {
        let mut frame_header = [0_u8; MANIFEST_FRAME_HEADER_SIZE];
        let read = read_up_to(&mut reader, &mut frame_header)
            .map_err(|source| Error::io("read_manifest_frame_header", Some(path.into()), source))?;
        if read == 0 {
            if !initialized {
                return Err(Error::corruption("MANIFEST", "не содержит VersionEdit"));
            }
            state.validate(limits)?;
            return Ok((state, summary));
        }
        if read != MANIFEST_FRAME_HEADER_SIZE {
            if !initialized {
                return Err(Error::corruption(
                    "MANIFEST",
                    "первый VersionEdit физически усечён",
                ));
            }
            summary.truncated_tail = true;
            state.validate(limits)?;
            return Ok((state, summary));
        }
        let descriptor = decode_frame_header(&frame_header, limits)?;
        let mut payload = vec![0_u8; descriptor.payload_length];
        let payload_read = read_up_to(&mut reader, &mut payload)
            .map_err(|source| Error::io("read_manifest_payload", Some(path.into()), source))?;
        if payload_read != descriptor.payload_length {
            if !initialized {
                return Err(Error::corruption(
                    "MANIFEST",
                    "первый VersionEdit физически усечён",
                ));
            }
            summary.truncated_tail = true;
            state.validate(limits)?;
            return Ok((state, summary));
        }
        let edit = decode_complete_frame(&frame_header, &payload, descriptor, limits)?;
        if !initialized {
            state = ManifestState {
                database_id,
                manifest_number,
                comparator: String::new(),
                log_number: INITIAL_LOG_NUMBER,
                previous_log_number: None,
                next_file_number: INITIAL_LOG_NUMBER + 1,
                last_sequence: None,
                l0_files: Vec::new(),
                minimum_reader_version: MINIMUM_READER_VERSION,
                feature_flags: 0,
            };
            initialized = true;
        }
        state = state.applied(&edit, limits)?;
        summary.edit_count = summary
            .edit_count
            .checked_add(1)
            .ok_or(Error::ResourceLimit {
                resource: "manifest_edit_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        summary.valid_bytes = summary
            .valid_bytes
            .checked_add(u64::try_from(descriptor.total_length).map_err(|_| {
                Error::ResourceLimit {
                    resource: "manifest_valid_bytes",
                    limit: u64::MAX,
                    observed: u64::MAX,
                }
            })?)
            .ok_or(Error::ResourceLimit {
                resource: "manifest_valid_bytes",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
    }
}

fn write_current<F: FileSystem>(filesystem: &F, directory: &Path, name: &str) -> Result<()> {
    if parse_manifest_file_name(name).is_none() {
        return Err(Error::invalid_argument(
            "manifest_name",
            "CURRENT принимает только canonical MANIFEST name",
        ));
    }
    let temporary = directory.join("CURRENT.tmp");
    let current = directory.join(CURRENT_FILE_NAME);
    let mut file = filesystem.create_writable(&temporary)?;
    file.write_all(name.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|source| Error::io("write_current", Some(temporary.clone()), source))?;
    file.sync_all()
        .map_err(|source| Error::io("sync_current", Some(temporary.clone()), source))?;
    drop(file);
    filesystem.rename_atomic(&temporary, &current)?;
    filesystem.sync_directory(directory)
}

fn read_current<F: FileSystem>(filesystem: &F, directory: &Path) -> Result<String> {
    let path = directory.join(CURRENT_FILE_NAME);
    let reader = filesystem.open_sequential(&path)?;
    let mut bytes = Vec::new();
    reader
        .take(1025)
        .read_to_end(&mut bytes)
        .map_err(|source| Error::io("read_current", Some(path.clone()), source))?;
    if bytes.len() > 1024 {
        return Err(Error::ResourceLimit {
            resource: "current_file_size",
            limit: 1024,
            observed: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    if bytes.is_empty() || !bytes.ends_with(b"\n") || bytes[..bytes.len() - 1].contains(&b'\n') {
        return Err(Error::corruption(
            "CURRENT",
            "файл должен содержать одну строку с завершающим LF",
        ));
    }
    let name = std::str::from_utf8(&bytes[..bytes.len() - 1])
        .map_err(|_| Error::corruption("CURRENT", "имя MANIFEST не UTF-8"))?;
    if name.contains('/') || name.contains('\\') || parse_manifest_file_name(name).is_none() {
        return Err(Error::corruption(
            "CURRENT",
            "имя MANIFEST не canonical или содержит путь",
        ));
    }
    Ok(name.to_owned())
}

fn encode_header(database_id: DatabaseId, manifest_number: u64) -> [u8; MANIFEST_HEADER_SIZE] {
    let mut output = [0_u8; MANIFEST_HEADER_SIZE];
    output[0..8].copy_from_slice(MANIFEST_MAGIC);
    output[8..10].copy_from_slice(&MANIFEST_FORMAT_VERSION.to_le_bytes());
    output[10..12].copy_from_slice(&(MANIFEST_HEADER_SIZE as u16).to_le_bytes());
    output[12..16].copy_from_slice(&0_u32.to_le_bytes());
    output[16..32].copy_from_slice(database_id.as_bytes());
    output[32..40].copy_from_slice(&manifest_number.to_le_bytes());
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    output[40..48].copy_from_slice(&created.to_le_bytes());
    output[48..52].copy_from_slice(&MINIMUM_READER_VERSION.to_le_bytes());
    output[52..60].copy_from_slice(&[0_u8; 8]);
    let checksum = crc32c(&output[..60]);
    output[60..64].copy_from_slice(&checksum.to_le_bytes());
    output
}

fn decode_header(
    input: &[u8; MANIFEST_HEADER_SIZE],
    database_id: DatabaseId,
    manifest_number: u64,
) -> Result<()> {
    if &input[0..8] != MANIFEST_MAGIC {
        return Err(Error::corruption("MANIFEST header", "неверная сигнатура"));
    }
    let version = read_u16(input, 8, "MANIFEST header")?;
    if version != MANIFEST_FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "MANIFEST",
            found: u32::from(version),
            minimum_supported: u32::from(MANIFEST_FORMAT_VERSION),
            maximum_supported: u32::from(MANIFEST_FORMAT_VERSION),
        });
    }
    if usize::from(read_u16(input, 10, "MANIFEST header")?) != MANIFEST_HEADER_SIZE
        || read_u32(input, 12, "MANIFEST header")? != 0
        || input[52..60].iter().any(|byte| *byte != 0)
    {
        return Err(Error::corruption(
            "MANIFEST header",
            "header length или reserved fields неверны",
        ));
    }
    if crc32c(&input[..60]) != read_u32(input, 60, "MANIFEST header")? {
        return Err(Error::corruption("MANIFEST header", "CRC32C не совпадает"));
    }
    let mut id = [0_u8; 16];
    id.copy_from_slice(&input[16..32]);
    if DatabaseId::from_bytes(id) != database_id {
        return Err(Error::corruption(
            "MANIFEST header",
            "database UUID не совпадает",
        ));
    }
    if read_u64(input, 32, "MANIFEST header")? != manifest_number {
        return Err(Error::corruption(
            "MANIFEST header",
            "manifest_number не совпадает с CURRENT",
        ));
    }
    let minimum = read_u32(input, 48, "MANIFEST header")?;
    if minimum > MINIMUM_READER_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "MANIFEST minimum reader version",
            found: minimum,
            minimum_supported: MINIMUM_READER_VERSION,
            maximum_supported: MINIMUM_READER_VERSION,
        });
    }
    Ok(())
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

fn read_u16(input: &[u8], offset: usize, component: &'static str) -> Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| Error::corruption(component, "u16 offset переполнен"))?;
    let bytes: [u8; 2] = input
        .get(offset..end)
        .ok_or_else(|| Error::corruption(component, "поле u16 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption(component, "поле u16 имеет неверную ширину"))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(input: &[u8], offset: usize, component: &'static str) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| Error::corruption(component, "u32 offset переполнен"))?;
    let bytes: [u8; 4] = input
        .get(offset..end)
        .ok_or_else(|| Error::corruption(component, "поле u32 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption(component, "поле u32 имеет неверную ширину"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &[u8], offset: usize, component: &'static str) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| Error::corruption(component, "u64 offset переполнен"))?;
    let bytes: [u8; 8] = input
        .get(offset..end)
        .ok_or_else(|| Error::corruption(component, "поле u64 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption(component, "поле u64 имеет неверную ширину"))?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
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
            "forgedb-manifest-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn create_append_and_reopen() -> Result<()> {
        let directory = temporary_directory("reopen")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let (mut writer, mut state) =
            create_manifest(&filesystem, &directory, database_id, 1, &limits)?;
        writer.append_edit(
            &mut state,
            &VersionEdit {
                log_number: Some(2),
                previous_log_number: Some(Some(1)),
                next_file_number: Some(3),
                ..VersionEdit::default()
            },
            &limits,
        )?;
        drop(writer);
        let opened = open_manifest(&filesystem, &directory, database_id, &limits, true)?;
        assert_eq!(opened.state.log_number, 2);
        assert_eq!(opened.state.previous_log_number, Some(1));
        assert_eq!(opened.replay.edit_count, 2);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn trailing_partial_edit_is_reported() -> Result<()> {
        let directory = temporary_directory("partial")?;
        let filesystem = NativeFileSystem;
        let database_id = DatabaseId::random();
        let limits = Limits::default();
        let (writer, _) = create_manifest(&filesystem, &directory, database_id, 1, &limits)?;
        let path = writer.path().to_path_buf();
        drop(writer);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|source| Error::io("open_test_manifest", Some(path.clone()), source))?;
        file.write_all(b"FMV1\x20")
            .map_err(|source| Error::io("append_partial_manifest", Some(path.clone()), source))?;
        drop(file);
        let opened = open_manifest(&filesystem, &directory, database_id, &limits, true)?;
        assert!(opened.replay.truncated_tail);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
