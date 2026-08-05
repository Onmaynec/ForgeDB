use std::path::{Path, PathBuf};

use forgedb_core::{Error, Result};
use forgedb_fs::{FileSystem, NativeFileSystem, read_identity};
use forgedb_manifest::{manifest_file_name, open_manifest};
use forgedb_wal::wal_file_name;

use crate::Options;
use crate::inventory::FileInventory;
use crate::version_set::VersionSet;

const LOCK_FILE_NAME: &str = "LOCK";
const ORPHAN_DIRECTORY_NAME: &str = ".forgedb-orphans";

/// Неразрушающий план normal recovery и безопасной уборки каталога.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryPlan {
    pub database_path: PathBuf,
    pub selected_manifest: String,
    pub replay_wals: Vec<String>,
    pub live_sstables: Vec<String>,
    pub obsolete_files: Vec<String>,
    pub orphan_files: Vec<String>,
    pub temporary_files: Vec<String>,
    pub backup_artifacts: Vec<String>,
    pub user_files: Vec<String>,
    pub unknown_files: Vec<String>,
    pub planned_safe_cleanup: Vec<String>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

impl RecoveryPlan {
    /// Анализирует authoritative metadata под process lock, не изменяя файлы базы.
    pub fn inspect(path: impl AsRef<Path>) -> Result<Self> {
        let database_path = path.as_ref().to_path_buf();
        if !database_path.exists() {
            return Err(Error::invalid_argument(
                "path",
                "каталог базы не существует",
            ));
        }

        let filesystem = NativeFileSystem;
        let _lock = filesystem.lock_exclusive(&database_path.join(LOCK_FILE_NAME))?;
        let identity = read_identity(&filesystem, &database_path)?;
        let options = Options::default();
        let limits = options.limits()?;
        let opened = open_manifest(&filesystem, &database_path, identity, &limits, true)?;
        let manifest = opened.state;
        let version = VersionSet::open(
            &database_path,
            identity,
            &manifest,
            &limits,
            options.level_count,
            options.max_compaction_bytes,
        )?;
        let live_numbers = version
            .files_in_read_order()?
            .into_iter()
            .map(|table| table.metadata().file_number)
            .collect::<Vec<_>>();
        let inventory = FileInventory::build(&database_path, &manifest, live_numbers)?;

        let mut replay_wals = Vec::with_capacity(2);
        if let Some(previous) = manifest.previous_log_number {
            replay_wals.push(wal_file_name(previous));
        }
        replay_wals.push(wal_file_name(manifest.log_number));

        Ok(Self::from_inventory(
            database_path,
            manifest_file_name(manifest.manifest_number),
            replay_wals,
            inventory,
        ))
    }

    fn from_inventory(
        database_path: PathBuf,
        selected_manifest: String,
        replay_wals: Vec<String>,
        inventory: FileInventory,
    ) -> Self {
        let mut planned_safe_cleanup =
            inventory
                .obsolete
                .iter()
                .map(|name| format!("удалить obsolete file после повторной проверки: {name}"))
                .chain(
                    inventory
                        .temporary
                        .iter()
                        .map(|name| format!("удалить temporary artifact после проверки: {name}")),
                )
                .chain(inventory.orphan_sst.iter().map(|name| {
                    format!("переместить orphan SST в {ORPHAN_DIRECTORY_NAME}/: {name}")
                }))
                .collect::<Vec<_>>();
        planned_safe_cleanup.sort();

        let mut warnings = Vec::new();
        if !inventory.orphan_sst.is_empty() {
            warnings.push("обнаружены orphan SSTables; они не входят в live state".to_owned());
        }
        if !inventory.backup_artifacts.is_empty() {
            warnings
                .push("обнаружены backup artifacts; normal recovery их не использует".to_owned());
        }
        if !inventory.user_files.is_empty() {
            warnings.push(
                "обнаружены явно помеченные user files; cleanup их не затрагивает".to_owned(),
            );
        }
        if !inventory.unknown.is_empty() {
            warnings.push("обнаружены unknown files; автоматическое удаление запрещено".to_owned());
        }

        Self {
            database_path,
            selected_manifest,
            replay_wals,
            live_sstables: inventory.live_sst,
            obsolete_files: inventory.obsolete,
            orphan_files: inventory.orphan_sst,
            temporary_files: inventory.temporary,
            backup_artifacts: inventory.backup_artifacts,
            user_files: inventory.user_files,
            unknown_files: inventory.unknown,
            planned_safe_cleanup,
            warnings,
            blockers: inventory.blockers,
        }
    }

    /// Возвращает true, если inventory не содержит blockers normal recovery.
    #[must_use]
    pub fn can_open_normally(&self) -> bool {
        self.blockers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::{Database, WriteOptions};

    use super::*;

    #[test]
    fn plan_reports_manifest_wal_and_sstable_without_mutation() -> Result<()> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "forgedb-recovery-plan-{}-{unique}",
            std::process::id()
        ));
        let database = Database::open(&directory, Options::default())?;
        database.put(b"key", b"value", WriteOptions::sync())?;
        database.flush()?;
        database.close()?;

        let before = fs::read_dir(&directory)
            .map_err(|source| Error::io("read_test_directory", Some(directory.clone()), source))?
            .count();
        let plan = RecoveryPlan::inspect(&directory)?;
        let after = fs::read_dir(&directory)
            .map_err(|source| Error::io("read_test_directory", Some(directory.clone()), source))?
            .count();

        assert!(plan.selected_manifest.starts_with("MANIFEST-"));
        assert_eq!(plan.replay_wals.len(), 1);
        assert_eq!(plan.live_sstables.len(), 1);
        assert!(plan.can_open_normally());
        assert_eq!(before, after);

        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
