use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb_core::{DatabaseId, Error, Limits, Result};
use forgedb_fs::{FileSystem, WritableFile};

use crate::state::ManifestState;
use crate::v2::{
    MANIFEST_V2_FORMAT_VERSION, ManifestStateV2, ManifestV2Codec, ManifestV2Header, VersionEditV2,
};
use crate::{
    CURRENT_FILE_NAME, MANIFEST_FRAME_HEADER_SIZE, MANIFEST_HEADER_SIZE, ReplaySummary,
    manifest_file_name, parse_manifest_file_name,
};

/// Append-only writer level-aware MANIFEST V2.
#[derive(Debug)]
pub struct ManifestV2Writer<W: WritableFile> {
    file: W,
    path: PathBuf,
    bytes_written: u64,
}

impl<W: WritableFile> ManifestV2Writer<W> {
    /// Preflight-применяет edit, durably append-ит frame и только затем публикует state.
    pub fn append_edit(
        &mut self,
        state: &mut ManifestStateV2,
        edit: &VersionEditV2,
        limits: &Limits,
    ) -> Result<()> {
        let next = state.applied(edit, limits)?;
        let frame = ManifestV2Codec::encode_edit_frame(edit, limits)?;
        self.file.write_all(&frame).map_err(|source| {
            Error::io("append_manifest_v2_edit", Some(self.path.clone()), source)
        })?;
        self.file
            .sync_all()
            .map_err(|source| Error::io("sync_manifest_v2", Some(self.path.clone()), source))?;
        self.bytes_written =
            checked_add_length(self.bytes_written, frame.len(), "manifest_v2_bytes_written")?;
        *state = next;
        Ok(())
    }

    pub fn sync(&self) -> Result<()> {
        self.file
            .sync_all()
            .map_err(|source| Error::io("sync_manifest_v2", Some(self.path.clone()), source))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

/// Результат открытия active MANIFEST V2.
#[derive(Debug)]
pub struct OpenManifestV2<W: WritableFile> {
    pub writer: Option<ManifestV2Writer<W>>,
    pub state: ManifestStateV2,
    pub replay: ReplaySummary,
}

/// Строит level-aware snapshot из authoritative V1 state без изменения V1 объекта.
pub fn migrated_v2_state(
    v1: &ManifestState,
    new_manifest_number: u64,
    level_count: usize,
    limits: &Limits,
) -> Result<ManifestStateV2> {
    v1.validate(limits)?;
    if new_manifest_number < v1.next_file_number {
        return Err(Error::invalid_argument(
            "new_manifest_number",
            "номер нового MANIFEST не может быть меньше next_file_number V1",
        ));
    }
    if new_manifest_number == 0 {
        return Err(Error::invalid_argument(
            "new_manifest_number",
            "номер нового MANIFEST не может быть нулём",
        ));
    }
    let next_file_number = new_manifest_number
        .checked_add(1)
        .ok_or(Error::ResourceLimit {
            resource: "file_number",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    let mut state = ManifestStateV2::initial(
        v1.database_id,
        new_manifest_number,
        v1.log_number,
        level_count,
    )?;
    state.comparator = v1.comparator.clone();
    state.previous_log_number = v1.previous_log_number;
    state.next_file_number = next_file_number;
    state.last_sequence = v1.last_sequence;
    let level_zero = state.levels.first_mut().ok_or(Error::InternalInvariant {
        invariant: "I-MANIFEST-V2-MIGRATION-L0",
        reason: "V2 state не содержит L0".to_owned(),
    })?;
    *level_zero = v1.l0_files.clone();
    state.validate(limits)?;
    Ok(state)
}

/// Создаёт новый V2 snapshot и crash-safe публикует его через CURRENT.
///
/// Старый V1 MANIFEST не изменяется. Temporary и final paths создаются только
/// если отсутствуют, поэтому подозрительный orphan никогда не перезаписывается.
pub fn migrate_manifest_v1_to_v2<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    v1: &ManifestState,
    new_manifest_number: u64,
    level_count: usize,
    limits: &Limits,
) -> Result<(ManifestV2Writer<F::WritableFile>, ManifestStateV2)> {
    limits.validate()?;
    let state = migrated_v2_state(v1, new_manifest_number, level_count, limits)?;
    let name = manifest_file_name(new_manifest_number);
    let temporary = directory.join(format!("{name}.tmp"));
    let final_path = directory.join(&name);
    ensure_path_absent(filesystem, &final_path, "MANIFEST V2 destination")?;
    let snapshot = ManifestV2Codec::encode_edit_frame(&state.snapshot_edit(), limits)?;
    let header = ManifestV2Codec::encode_header(ManifestV2Header {
        database_id: state.database_id,
        manifest_number: state.manifest_number,
        created_unix_ms: unix_time_millis(),
    })?;

    let mut file = filesystem.create_new_writable(&temporary)?;
    file.write_all(&header)
        .and_then(|_| file.write_all(&snapshot))
        .map_err(|source| {
            Error::io(
                "write_manifest_v2_snapshot",
                Some(temporary.clone()),
                source,
            )
        })?;
    file.sync_all().map_err(|source| {
        Error::io("sync_manifest_v2_snapshot", Some(temporary.clone()), source)
    })?;
    drop(file);

    filesystem.rename_atomic(&temporary, &final_path)?;
    filesystem.sync_directory(directory)?;
    write_current_v2(filesystem, directory, &name)?;

    let bytes_written = checked_add_length(
        u64::try_from(MANIFEST_HEADER_SIZE).map_err(|_| Error::ResourceLimit {
            resource: "manifest_v2_bytes_written",
            limit: u64::MAX,
            observed: u64::MAX,
        })?,
        snapshot.len(),
        "manifest_v2_bytes_written",
    )?;
    Ok((
        ManifestV2Writer {
            file: filesystem.open_appendable(&final_path)?,
            path: final_path,
            bytes_written,
        },
        state,
    ))
}

/// Открывает active V2 MANIFEST из CURRENT.
pub fn open_manifest_v2<F: FileSystem>(
    filesystem: &F,
    directory: &Path,
    database_id: DatabaseId,
    limits: &Limits,
    read_only: bool,
) -> Result<OpenManifestV2<F::WritableFile>> {
    limits.validate()?;
    let name = read_current_v2(filesystem, directory)?;
    let number = parse_manifest_file_name(&name)
        .ok_or_else(|| Error::corruption("CURRENT", "имя MANIFEST не canonical"))?;
    let path = directory.join(name);
    let (state, summary) = replay_manifest_v2(filesystem, &path, database_id, number, limits)?;
    if summary.truncated_tail && !read_only {
        filesystem.truncate_file(&path, summary.valid_bytes)?;
    }
    let writer = if read_only {
        None
    } else {
        Some(ManifestV2Writer {
            file: filesystem.open_appendable(&path)?,
            path,
            bytes_written: summary.valid_bytes,
        })
    };
    Ok(OpenManifestV2 {
        writer,
        state,
        replay: summary,
    })
}

/// Строго replayed complete V2 edits; допускается только последний partial frame.
pub fn replay_manifest_v2<F: FileSystem>(
    filesystem: &F,
    path: &Path,
    database_id: DatabaseId,
    manifest_number: u64,
    limits: &Limits,
) -> Result<(ManifestStateV2, ReplaySummary)> {
    limits.validate()?;
    let mut reader = filesystem.open_sequential(path)?;
    let mut header_bytes = [0_u8; MANIFEST_HEADER_SIZE];
    if read_up_to(&mut reader, &mut header_bytes)
        .map_err(|source| Error::io("read_manifest_v2_header", Some(path.into()), source))?
        != MANIFEST_HEADER_SIZE
    {
        return Err(Error::corruption(
            "MANIFEST V2 header",
            "файл усечён до конца заголовка",
        ));
    }
    let header = ManifestV2Codec::decode_header(&header_bytes)?;
    if header.database_id != database_id {
        return Err(Error::corruption(
            "MANIFEST V2 header",
            "database UUID не совпадает",
        ));
    }
    if header.manifest_number != manifest_number {
        return Err(Error::corruption(
            "MANIFEST V2 header",
            "manifest_number не совпадает с CURRENT",
        ));
    }

    let mut state: Option<ManifestStateV2> = None;
    let mut summary = ReplaySummary {
        valid_bytes: u64::try_from(MANIFEST_HEADER_SIZE).unwrap_or(u64::MAX),
        ..ReplaySummary::default()
    };
    loop {
        let mut frame_header = [0_u8; MANIFEST_FRAME_HEADER_SIZE];
        let read = read_up_to(&mut reader, &mut frame_header).map_err(|source| {
            Error::io("read_manifest_v2_frame_header", Some(path.into()), source)
        })?;
        if read == 0 {
            let state = state
                .ok_or_else(|| Error::corruption("MANIFEST V2", "не содержит VersionEdit V2"))?;
            state.validate(limits)?;
            return Ok((state, summary));
        }
        if read != MANIFEST_FRAME_HEADER_SIZE {
            let state = state.ok_or_else(|| {
                Error::corruption("MANIFEST V2", "первый VersionEdit V2 физически усечён")
            })?;
            summary.truncated_tail = true;
            state.validate(limits)?;
            return Ok((state, summary));
        }
        let payload_length = bounded_payload_length(&frame_header, limits)?;
        let total_length = MANIFEST_FRAME_HEADER_SIZE
            .checked_add(payload_length)
            .ok_or_else(|| Error::corruption("MANIFEST V2 frame", "длина переполнена"))?;
        let mut payload = vec![0_u8; payload_length];
        let payload_read = read_up_to(&mut reader, &mut payload)
            .map_err(|source| Error::io("read_manifest_v2_payload", Some(path.into()), source))?;
        if payload_read != payload_length {
            let state = state.ok_or_else(|| {
                Error::corruption("MANIFEST V2", "первый VersionEdit V2 физически усечён")
            })?;
            summary.truncated_tail = true;
            state.validate(limits)?;
            return Ok((state, summary));
        }
        let mut frame = Vec::with_capacity(total_length);
        frame.extend_from_slice(&frame_header);
        frame.extend_from_slice(&payload);
        let edit = ManifestV2Codec::decode_edit_frame(&frame, limits)?;
        state = Some(match state {
            None => state_from_snapshot(database_id, manifest_number, &edit, limits)?,
            Some(current) => current.applied(&edit, limits)?,
        });
        summary.edit_count = summary
            .edit_count
            .checked_add(1)
            .ok_or(Error::ResourceLimit {
                resource: "manifest_v2_edit_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        summary.valid_bytes =
            checked_add_length(summary.valid_bytes, total_length, "manifest_v2_valid_bytes")?;
    }
}

fn state_from_snapshot(
    database_id: DatabaseId,
    manifest_number: u64,
    edit: &VersionEditV2,
    limits: &Limits,
) -> Result<ManifestStateV2> {
    let comparator = edit
        .comparator
        .as_ref()
        .ok_or_else(|| Error::corruption("MANIFEST V2 snapshot", "comparator отсутствует"))?;
    if comparator.is_empty() {
        return Err(Error::corruption("MANIFEST V2 snapshot", "comparator пуст"));
    }
    let log_number = edit
        .log_number
        .ok_or_else(|| Error::corruption("MANIFEST V2 snapshot", "log_number отсутствует"))?;
    if edit.previous_log_number.is_none()
        || edit.next_file_number.is_none()
        || edit.last_sequence.is_none()
        || edit.minimum_reader_version.is_none()
        || edit.feature_flags.is_none()
    {
        return Err(Error::corruption(
            "MANIFEST V2 snapshot",
            "обязательные snapshot fields отсутствуют",
        ));
    }
    let level_count_u32 = edit
        .level_count
        .ok_or_else(|| Error::corruption("MANIFEST V2 snapshot", "level_count отсутствует"))?;
    let level_count = usize::try_from(level_count_u32).map_err(|_| Error::ResourceLimit {
        resource: "manifest_v2_level_count",
        limit: u64::MAX,
        observed: u64::from(level_count_u32),
    })?;
    if !edit.deleted_files.is_empty() {
        return Err(Error::corruption(
            "MANIFEST V2 snapshot",
            "первый snapshot edit не может удалять files",
        ));
    }
    let initial = ManifestStateV2::initial(database_id, manifest_number, log_number, level_count)?;
    initial.applied(edit, limits)
}

fn bounded_payload_length(
    frame_header: &[u8; MANIFEST_FRAME_HEADER_SIZE],
    limits: &Limits,
) -> Result<usize> {
    if frame_header.get(0..4) != Some(b"FMV2".as_slice()) {
        return Err(Error::corruption("MANIFEST V2 frame", "неверная сигнатура"));
    }
    let version = read_u16(frame_header, 14, "MANIFEST V2 frame")?;
    if version != MANIFEST_V2_FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            component: "MANIFEST V2 frame",
            found: u32::from(version),
            minimum_supported: u32::from(MANIFEST_V2_FORMAT_VERSION),
            maximum_supported: u32::from(MANIFEST_V2_FORMAT_VERSION),
        });
    }
    let total_u32 = read_u32(frame_header, 4, "MANIFEST V2 frame")?;
    let payload_u32 = read_u32(frame_header, 20, "MANIFEST V2 frame")?;
    let payload = usize::try_from(payload_u32).map_err(|_| Error::ResourceLimit {
        resource: "manifest_v2_payload_length",
        limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
        observed: u64::from(payload_u32),
    })?;
    if payload > limits.max_batch_size {
        return Err(Error::ResourceLimit {
            resource: "manifest_v2_payload_length",
            limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
            observed: u64::from(payload_u32),
        });
    }
    let expected = MANIFEST_FRAME_HEADER_SIZE
        .checked_add(payload)
        .ok_or_else(|| Error::corruption("MANIFEST V2 frame", "длина переполнена"))?;
    if u64::try_from(expected).unwrap_or(u64::MAX) != u64::from(total_u32) {
        return Err(Error::corruption(
            "MANIFEST V2 frame",
            "total length не совпадает с payload length",
        ));
    }
    Ok(payload)
}

fn ensure_path_absent<F: FileSystem>(
    filesystem: &F,
    path: &Path,
    component: &'static str,
) -> Result<()> {
    match filesystem.file_size(path) {
        Ok(size) => Err(Error::corruption(
            component,
            format!(
                "destination уже существует: {} ({size} bytes)",
                path.display()
            ),
        )),
        Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_current_v2<F: FileSystem>(filesystem: &F, directory: &Path, name: &str) -> Result<()> {
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
        .map_err(|source| Error::io("write_current_v2", Some(temporary.clone()), source))?;
    file.sync_all()
        .map_err(|source| Error::io("sync_current_v2", Some(temporary.clone()), source))?;
    drop(file);
    filesystem.rename_atomic(&temporary, &current)?;
    filesystem.sync_directory(directory)
}

fn read_current_v2<F: FileSystem>(filesystem: &F, directory: &Path) -> Result<String> {
    let path = directory.join(CURRENT_FILE_NAME);
    let reader = filesystem.open_sequential(&path)?;
    let mut bytes = Vec::new();
    reader
        .take(1025)
        .read_to_end(&mut bytes)
        .map_err(|source| Error::io("read_current_v2", Some(path.clone()), source))?;
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

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn checked_add_length(left: u64, right: usize, resource: &'static str) -> Result<u64> {
    let right = u64::try_from(right).map_err(|_| Error::ResourceLimit {
        resource,
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    left.checked_add(right).ok_or(Error::ResourceLimit {
        resource,
        limit: u64::MAX,
        observed: u64::MAX,
    })
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    use forgedb_core::SequenceNumber;
    use forgedb_fs::NativeFileSystem;
    use forgedb_sstable::TableMeta;

    use super::*;
    use crate::{LevelFile, VersionEdit};

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-manifest-v2-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    fn table(number: u64, smallest: &[u8], largest: &[u8]) -> TableMeta {
        TableMeta {
            file_number: number,
            file_size: 256,
            smallest_user_key: smallest.to_vec(),
            largest_user_key: largest.to_vec(),
            smallest_sequence: SequenceNumber::FIRST,
            largest_sequence: SequenceNumber::FIRST,
            entry_count: 1,
        }
    }

    fn v1_state(database_id: DatabaseId, limits: &Limits) -> Result<ManifestState> {
        let initial = ManifestState::initial(database_id, 1)?;
        initial.applied(
            &VersionEdit {
                next_file_number: Some(11),
                last_sequence: Some(Some(SequenceNumber::FIRST)),
                added_files: vec![table(10, b"a", b"z")],
                ..VersionEdit::default()
            },
            limits,
        )
    }

    #[test]
    fn migration_publishes_v2_without_mutating_v1_state() -> Result<()> {
        let directory = temporary_directory("migration")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let v1 = v1_state(database_id, &limits)?;
        let original = v1.clone();

        let (writer, migrated) =
            migrate_manifest_v1_to_v2(&filesystem, &directory, &v1, 11, 4, &limits)?;
        assert_eq!(v1, original);
        assert_eq!(migrated.manifest_number, 11);
        assert_eq!(migrated.next_file_number, 12);
        assert_eq!(migrated.levels[0], v1.l0_files);
        assert!(migrated.levels[1..].iter().all(Vec::is_empty));
        assert_eq!(
            fs::read_to_string(directory.join(CURRENT_FILE_NAME)).map_err(|source| Error::io(
                "read_current_test",
                Some(directory.clone()),
                source
            ))?,
            "MANIFEST-000011\n"
        );
        drop(writer);

        let opened = open_manifest_v2(&filesystem, &directory, database_id, &limits, true)?;
        assert_eq!(opened.state, migrated);
        assert_eq!(opened.replay.edit_count, 1);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn append_level_edit_survives_reopen() -> Result<()> {
        let directory = temporary_directory("append")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let v1 = v1_state(database_id, &limits)?;
        let (mut writer, mut state) =
            migrate_manifest_v1_to_v2(&filesystem, &directory, &v1, 11, 4, &limits)?;
        writer.append_edit(
            &mut state,
            &VersionEditV2 {
                next_file_number: Some(13),
                added_files: vec![LevelFile {
                    level: 1,
                    metadata: table(12, b"aa", b"az"),
                }],
                ..VersionEditV2::default()
            },
            &limits,
        )?;
        drop(writer);
        let opened = open_manifest_v2(&filesystem, &directory, database_id, &limits, true)?;
        assert_eq!(opened.state, state);
        assert_eq!(opened.state.levels[1].len(), 1);
        assert_eq!(opened.replay.edit_count, 2);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn trailing_partial_v2_edit_is_reported_and_truncated_on_rw_open() -> Result<()> {
        let directory = temporary_directory("partial")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let v1 = v1_state(database_id, &limits)?;
        let (writer, _) = migrate_manifest_v1_to_v2(&filesystem, &directory, &v1, 11, 4, &limits)?;
        let path = writer.path().to_path_buf();
        let valid_bytes = writer.bytes_written();
        drop(writer);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|source| Error::io("open_manifest_v2_test", Some(path.clone()), source))?;
        file.write_all(b"FMV2\x20").map_err(|source| {
            Error::io("append_partial_manifest_v2", Some(path.clone()), source)
        })?;
        drop(file);

        let read_only = open_manifest_v2(&filesystem, &directory, database_id, &limits, true)?;
        assert!(read_only.replay.truncated_tail);
        drop(read_only);
        let read_write = open_manifest_v2(&filesystem, &directory, database_id, &limits, false)?;
        assert!(read_write.replay.truncated_tail);
        assert_eq!(filesystem.file_size(&path)?, valid_bytes);
        drop(read_write);
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn migration_refuses_to_overwrite_existing_destination() -> Result<()> {
        let directory = temporary_directory("existing")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let v1 = v1_state(database_id, &limits)?;
        let temporary = directory.join("MANIFEST-000011.tmp");
        fs::write(&temporary, b"suspicious").map_err(|source| {
            Error::io(
                "write_existing_manifest_test",
                Some(temporary.clone()),
                source,
            )
        })?;
        assert!(migrate_manifest_v1_to_v2(&filesystem, &directory, &v1, 11, 4, &limits,).is_err());
        assert_eq!(
            fs::read(&temporary).map_err(|source| Error::io(
                "read_existing_manifest_test",
                Some(temporary.clone()),
                source
            ))?,
            b"suspicious"
        );
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }

    #[test]
    fn migration_refuses_to_overwrite_existing_final_manifest() -> Result<()> {
        let directory = temporary_directory("existing-final")?;
        let filesystem = NativeFileSystem;
        let limits = Limits::default();
        let database_id = DatabaseId::random();
        let v1 = v1_state(database_id, &limits)?;
        let final_path = directory.join("MANIFEST-000011");
        let temporary = directory.join("MANIFEST-000011.tmp");
        fs::write(&final_path, b"suspicious-final").map_err(|source| {
            Error::io(
                "write_existing_final_manifest_test",
                Some(final_path.clone()),
                source,
            )
        })?;
        assert!(migrate_manifest_v1_to_v2(&filesystem, &directory, &v1, 11, 4, &limits,).is_err());
        assert_eq!(
            fs::read(&final_path).map_err(|source| Error::io(
                "read_existing_final_manifest_test",
                Some(final_path.clone()),
                source,
            ))?,
            b"suspicious-final"
        );
        assert!(!temporary.exists());
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
