//! Portable logical archive v1: independently recoverable default/named CF keyspaces.
//!
//! This is NOT a copy of a running WAL/SST/MANIFEST and NOT a cross-family
//! atomic snapshot. Each family has its own MVCC snapshot. Restore is published
//! only after checksum validation and successful close in a NEW directory.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use forgedb::{ColumnFamilies, ColumnFamilyId, Error, Options, ReadOptions, Result, WriteOptions};

const MAGIC: &[u8; 8] = b"FGDBBK01";
const FORMAT_VERSION: u32 = 1;
const FAMILY_START: u8 = 1;
const ENTRY: u8 = 2;
const FAMILY_END: u8 = 3;
const ARCHIVE_END: u8 = 255;
const MAX_ARCHIVE_FAMILIES: u32 = 1025;
const MAX_FAMILY_NAME_BYTES: usize = 64;

const fn crc32c_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ 0x82f6_3b78
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[i] = value;
        i += 1;
    }
    table
}
const CRC32C_TABLE: [u32; 256] = crc32c_table();

fn update_crc(crc: &mut u32, bytes: &[u8]) {
    for &byte in bytes {
        let index = ((*crc as u8) ^ byte) as usize;
        *crc = CRC32C_TABLE[index] ^ (*crc >> 8);
    }
}

struct ArchiveWriter {
    output: BufWriter<File>,
    crc: u32,
}

impl ArchiveWriter {
    fn new(output: File) -> Self {
        Self {
            output: BufWriter::new(output),
            crc: !0,
        }
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.output
            .write_all(bytes)
            .map_err(|source| Error::io("write_archive", None, source))?;
        update_crc(&mut self.crc, bytes);
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<()> {
        self.bytes(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    fn finish(mut self) -> Result<()> {
        self.u8(ARCHIVE_END)?;
        self.output
            .write_all(&(!self.crc).to_le_bytes())
            .map_err(|source| Error::io("write_archive_checksum", None, source))?;
        self.output
            .flush()
            .map_err(|source| Error::io("flush_archive", None, source))?;
        let file = self
            .output
            .into_inner()
            .map_err(|error| Error::io("finish_archive", None, error.into_error()))?;
        file.sync_all()
            .map_err(|source| Error::io("sync_archive", None, source))
    }
}

struct ArchiveReader {
    input: BufReader<File>,
    crc: u32,
}

impl ArchiveReader {
    fn new(input: File) -> Self {
        Self {
            input: BufReader::new(input),
            crc: !0,
        }
    }

    fn bytes(&mut self, output: &mut [u8]) -> Result<()> {
        self.input.read_exact(output).map_err(|source| {
            if source.kind() == io::ErrorKind::UnexpectedEof {
                Error::corruption("logical archive", "преждевременное завершение файла")
            } else {
                Error::io("read_archive", None, source)
            }
        })?;
        update_crc(&mut self.crc, output);
        Ok(())
    }

    fn u8(&mut self) -> Result<u8> {
        let mut value = [0];
        self.bytes(&mut value)?;
        Ok(value[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let mut value = [0; 2];
        self.bytes(&mut value)?;
        Ok(u16::from_le_bytes(value))
    }

    fn u32(&mut self) -> Result<u32> {
        let mut value = [0; 4];
        self.bytes(&mut value)?;
        Ok(u32::from_le_bytes(value))
    }

    fn vector(&mut self, size: usize, maximum: usize) -> Result<Vec<u8>> {
        if size > maximum {
            return Err(Error::ResourceLimit {
                resource: "logical_archive_record_bytes",
                limit: u64::try_from(maximum).unwrap_or(u64::MAX),
                observed: u64::try_from(size).unwrap_or(u64::MAX),
            });
        }
        let mut value = vec![0; size];
        self.bytes(&mut value)?;
        Ok(value)
    }

    fn header(&mut self) -> Result<u32> {
        let mut magic = [0; 8];
        self.bytes(&mut magic)?;
        if &magic != MAGIC {
            return Err(Error::corruption("logical archive", "неверный magic"));
        }
        let version = self.u32()?;
        if version != FORMAT_VERSION {
            return Err(Error::UnsupportedFormat {
                component: "ForgeDB logical backup",
                found: version,
                minimum_supported: FORMAT_VERSION,
                maximum_supported: FORMAT_VERSION,
            });
        }
        let count = self.u32()?;
        if !(1..=MAX_ARCHIVE_FAMILIES).contains(&count) {
            return Err(Error::corruption(
                "logical archive",
                "недопустимое число Column Families",
            ));
        }
        Ok(count)
    }

    fn finish(&mut self) -> Result<()> {
        if self.u8()? != ARCHIVE_END {
            return Err(Error::corruption("logical archive", "отсутствует завершающий marker"));
        }
        let actual = !self.crc;
        let mut expected = [0_u8; 4];
        self.input
            .read_exact(&mut expected)
            .map_err(|source| Error::io("read_archive_checksum", None, source))?;
        if actual != u32::from_le_bytes(expected) {
            return Err(Error::corruption(
                "logical archive",
                "CRC32C checksum не совпадает",
            ));
        }
        let mut trailing = [0];
        if self
            .input
            .read(&mut trailing)
            .map_err(|source| Error::io("read_archive_trailing", None, source))?
            != 0
        {
            return Err(Error::corruption("logical archive", "лишние байты после checksum"));
        }
        Ok(())
    }
}

fn exclusive_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| Error::io("create_archive_temp", Some(path.to_owned()), source))
}

fn temporary_path(path: &Path, suffix: &str) -> Result<PathBuf> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(Error::invalid_argument("path", "родительский каталог не существует"));
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(parent.join(format!(
        ".forgedb-{suffix}-{}-{stamp}",
        std::process::id()
    )))
}

fn path_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::io("stat_archive_path", Some(path.to_owned()), source)),
    }
}

/// Export all registered families, each at its own independent MVCC snapshot.
/// The output is created through same-directory temporary + hard_link,
/// preventing overwrite of an existing user file even under a race.
pub(crate) fn export(database_path: &Path, archive_path: &Path) -> Result<()> {
    if path_present(archive_path)? {
        return Err(Error::invalid_argument("archive", "файл архива уже существует"));
    }
    let families = ColumnFamilies::open(
        database_path,
        Options {
            read_only: true,
            create_if_missing: false,
            ..Options::default()
        },
    )?;
    let entries = families.list()?;
    let count = u32::try_from(entries.len()).map_err(|_| Error::ResourceLimit {
        resource: "logical_archive_families",
        limit: u64::from(MAX_ARCHIVE_FAMILIES),
        observed: u64::try_from(entries.len()).unwrap_or(u64::MAX),
    })?;

    // Pin each family at a self-consistent snapshot before copying any records.
    // This is not an atomic snapshot across families.
    let mut snapshots = Vec::with_capacity(entries.len());
    for info in entries {
        let snapshot = families.snapshot(info.id)?;
        snapshots.push((info, snapshot));
    }

    let temp = temporary_path(archive_path, "export")?;
    let result = (|| -> Result<()> {
        let file = exclusive_file(&temp)?;
        let mut archive = ArchiveWriter::new(file);
        archive.bytes(MAGIC)?;
        archive.u32(FORMAT_VERSION)?;
        archive.u32(count)?;
        for (info, snapshot) in &snapshots {
            archive.u8(FAMILY_START)?;
            let name = info.name.as_bytes();
            let name_size = u16::try_from(name.len()).map_err(|_| Error::ResourceLimit {
                resource: "logical_archive_family_name",
                limit: u64::try_from(MAX_FAMILY_NAME_BYTES).unwrap_or(u64::MAX),
                observed: u64::try_from(name.len()).unwrap_or(u64::MAX),
            })?;
            archive.u16(name_size)?;
            archive.bytes(name)?;
            for row in families.scan(info.id, .., ReadOptions::from_snapshot(snapshot))? {
                let (key, value) = row?;
                archive.u8(ENTRY)?;
                let key_len = u32::try_from(key.len()).map_err(|_| Error::ResourceLimit {
                    resource: "logical_archive_key_length",
                    limit: u64::from(u32::MAX),
                    observed: u64::try_from(key.len()).unwrap_or(u64::MAX),
                })?;
                let val_len = u32::try_from(value.len()).map_err(|_| Error::ResourceLimit {
                    resource: "logical_archive_value_length",
                    limit: u64::from(u32::MAX),
                    observed: u64::try_from(value.len()).unwrap_or(u64::MAX),
                })?;
                archive.u32(key_len)?;
                archive.u32(val_len)?;
                archive.bytes(&key)?;
                archive.bytes(&value)?;
            }
            archive.u8(FAMILY_END)?;
        }
        archive.finish()?;
        families.close()?;
        // Unlike rename, hard_link cannot replace a pre-existing destination.
        // The staging and target share one parent/filesystem.
        fs::hard_link(&temp, archive_path)
            .map_err(|source| Error::io("publish_archive", Some(archive_path.to_owned()), source))?;
        Ok(())
    })();
    let cleanup = fs::remove_file(&temp);
    result?;
    cleanup.map_err(|source| Error::io("cleanup_archive_temp", Some(temp), source))?;
    println!("logical archive: {}", archive_path.display());
    println!("families: {count}");
    Ok(())
}

/// Restore into a fresh database directory. A corrupt/truncated/oversized
/// archive never becomes visible at target; only a validated and closed staging
/// database is renamed into place.
pub(crate) fn restore(archive_path: &Path, target: &Path) -> Result<()> {
    if path_present(target)? {
        return Err(Error::invalid_argument("destination", "каталог назначения уже существует"));
    }
    let input = File::open(archive_path)
        .map_err(|source| Error::io("open_archive", Some(archive_path.to_owned()), source))?;
    let mut archive = ArchiveReader::new(input);
    let families_count = archive.header()?;
    let temp = temporary_path(target, "restore")?;
    fs::create_dir(&temp)
        .map_err(|source| Error::io("create_restore_stage", Some(temp.clone()), source))?;
    let result = (|| -> Result<()> {
        let families = ColumnFamilies::open(&temp, Options::default())?;
        let import = (|| -> Result<()> {
            let opts = Options::default();
            let mut names = std::collections::BTreeSet::new();
            for index in 0..families_count {
                if archive.u8()? != FAMILY_START {
                    return Err(Error::corruption("logical archive", "ожидалась запись family"));
                }
                let name_size = usize::from(archive.u16()?);
                let name = archive.vector(name_size, MAX_FAMILY_NAME_BYTES)?;
                let name = String::from_utf8(name).map_err(|_| {
                    Error::corruption("logical archive", "имя family не является UTF-8")
                })?;
                if !names.insert(name.clone()) {
                    return Err(Error::corruption("logical archive", "дублирование имени family"));
                }
                let id = if index == 0 {
                    if name != "default" {
                        return Err(Error::corruption(
                            "logical archive",
                            "первая family должна быть default",
                        ));
                    }
                    ColumnFamilyId::DEFAULT
                } else {
                    if name == "default" {
                        return Err(Error::corruption("logical archive", "повторная default family"));
                    }
                    families.create(&name)?
                };
                loop {
                    match archive.u8()? {
                        ENTRY => {
                            let key_size = usize::try_from(archive.u32()?).map_err(|_| {
                                Error::corruption("logical archive", "key size переполняет usize")
                            })?;
                            let value_size = usize::try_from(archive.u32()?).map_err(|_| {
                                Error::corruption("logical archive", "value size переполняет usize")
                            })?;
                            let key = archive.vector(key_size, opts.max_key_size)?;
                            let value = archive.vector(value_size, opts.max_value_size)?;
                            families.put(id, key, value, WriteOptions::no_sync())?;
                        }
                        FAMILY_END => break,
                        _ => {
                            return Err(Error::corruption(
                                "logical archive",
                                "неизвестный тип записи",
                            ));
                        }
                    }
                }
            }
            archive.finish()?;
            Ok(())
        })();
        let closing = families.close();
        import?;
        closing?;
        if path_present(target)? {
            return Err(Error::invalid_argument("destination", "назначение появилось во время restore"));
        }
        fs::rename(&temp, target)
            .map_err(|source| Error::io("publish_restored_database", Some(target.to_owned()), source))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
    }
    result?;
    println!("restored database: {}", target.display());
    println!("families: {families_count}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_castagnoli_known_vector() {
        let mut crc = !0_u32;
        update_crc(&mut crc, b"123456789");
        assert_eq!(!crc, 0xe306_9283);
    }
}
