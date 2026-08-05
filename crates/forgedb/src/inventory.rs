use std::collections::BTreeSet;
use std::path::Path;

use forgedb_core::Result;
use forgedb_fs::{FileSystem, IDENTITY_FILE_NAME, NativeFileSystem};
use forgedb_manifest::{
    CURRENT_FILE_NAME, ManifestState, manifest_file_name, parse_manifest_file_name,
};
use forgedb_sstable::parse_sst_file_name;
use forgedb_wal::{parse_wal_file_name, wal_file_name};

const LOCK_FILE_NAME: &str = "LOCK";
const TEMP_DIRECTORY_NAME: &str = "TEMP";
const ORPHAN_DIRECTORY_NAME: &str = ".forgedb-orphans";
const USER_DIRECTORY_NAME: &str = ".forgedb-user";

/// Неразрушающая классификация файлов каталога базы.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileInventory {
    pub live_metadata: Vec<String>,
    pub live_wal: Vec<String>,
    pub live_sst: Vec<String>,
    pub obsolete: Vec<String>,
    pub orphan_sst: Vec<String>,
    pub temporary: Vec<String>,
    pub backup_artifacts: Vec<String>,
    pub user_files: Vec<String>,
    pub unknown: Vec<String>,
    pub blockers: Vec<String>,
}

impl FileInventory {
    pub(crate) fn build(
        directory: &Path,
        manifest: &ManifestState,
        live_sst_numbers: impl IntoIterator<Item = u64>,
    ) -> Result<Self> {
        let filesystem = NativeFileSystem;
        let mut inventory = Self::default();
        let live_sst_numbers = live_sst_numbers.into_iter().collect::<BTreeSet<_>>();
        let active_manifest = manifest_file_name(manifest.manifest_number);
        let current_wal = wal_file_name(manifest.log_number);
        let previous_wal = manifest.previous_log_number.map(wal_file_name);
        let mut seen = BTreeSet::<String>::new();

        for path in filesystem.list_dir(directory)? {
            let Some(file_name) = path.file_name() else {
                inventory.unknown.push(path.display().to_string());
                continue;
            };
            let name = file_name.to_string_lossy().into_owned();
            seen.insert(name.clone());

            let is_live_metadata = name == IDENTITY_FILE_NAME
                || name == CURRENT_FILE_NAME
                || name == LOCK_FILE_NAME
                || name == ORPHAN_DIRECTORY_NAME
                || name == active_manifest;
            if is_live_metadata {
                inventory.live_metadata.push(name);
                continue;
            }
            if name == current_wal || previous_wal.as_ref() == Some(&name) {
                inventory.live_wal.push(name);
                continue;
            }
            if name == TEMP_DIRECTORY_NAME || name.ends_with(".tmp") {
                inventory.temporary.push(name);
                continue;
            }
            if let Some(number) = parse_sst_file_name(&name) {
                if live_sst_numbers.contains(&number) {
                    inventory.live_sst.push(name);
                } else {
                    inventory.orphan_sst.push(name);
                }
                continue;
            }
            if parse_wal_file_name(&name).is_some() || parse_manifest_file_name(&name).is_some() {
                inventory.obsolete.push(name);
                continue;
            }
            if is_backup_artifact(&name) {
                inventory.backup_artifacts.push(name);
                continue;
            }
            if is_explicit_user_file(&name) {
                inventory.user_files.push(name);
                continue;
            }
            inventory.unknown.push(name);
        }

        for required in [
            IDENTITY_FILE_NAME.to_owned(),
            CURRENT_FILE_NAME.to_owned(),
            active_manifest,
            current_wal,
        ] {
            if !seen.contains(&required) {
                inventory
                    .blockers
                    .push(format!("required file отсутствует: {required}"));
            }
        }
        if let Some(previous) = previous_wal
            && !seen.contains(&previous)
        {
            inventory
                .blockers
                .push(format!("previous WAL отсутствует: {previous}"));
        }
        for number in live_sst_numbers {
            let required = forgedb_sstable::sst_file_name(number);
            if !seen.contains(&required) {
                inventory
                    .blockers
                    .push(format!("live SSTable отсутствует: {required}"));
            }
        }

        inventory.sort();
        Ok(inventory)
    }

    fn sort(&mut self) {
        self.live_metadata.sort();
        self.live_wal.sort();
        self.live_sst.sort();
        self.obsolete.sort();
        self.orphan_sst.sort();
        self.temporary.sort();
        self.backup_artifacts.sort();
        self.user_files.sort();
        self.unknown.sort();
        self.blockers.sort();
    }
}

fn is_backup_artifact(name: &str) -> bool {
    name.starts_with("BACKUP-")
        || name.ends_with(".bak")
        || name.ends_with(".backup")
        || name.ends_with(".old")
        || name.ends_with('~')
}

fn is_explicit_user_file(name: &str) -> bool {
    name == USER_DIRECTORY_NAME || name.starts_with("USER-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_and_user_names_are_explicit() {
        assert!(is_backup_artifact("CURRENT.bak"));
        assert!(is_backup_artifact("BACKUP-20260804"));
        assert!(!is_backup_artifact("notes.txt"));
        assert!(is_explicit_user_file("USER-notes.txt"));
        assert!(is_explicit_user_file(USER_DIRECTORY_NAME));
        assert!(!is_explicit_user_file("notes.txt"));
    }
}
