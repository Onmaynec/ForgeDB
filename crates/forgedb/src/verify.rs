use forgedb_core::{DatabaseId, Error, Limits, Result, SequenceNumber};

use crate::inventory::FileInventory;
use crate::table_scan;
use crate::version_set::VersionSet;

/// Режим проверки открытой базы.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifyOptions {
    /// Читать и декодировать каждый live SSTable block.
    pub deep: bool,
    /// Проверять CRC32C persisted blocks.
    pub verify_checksums: bool,
}

impl VerifyOptions {
    #[must_use]
    pub const fn quick() -> Self {
        Self {
            deep: false,
            verify_checksums: true,
        }
    }

    #[must_use]
    pub const fn deep() -> Self {
        Self {
            deep: true,
            verify_checksums: true,
        }
    }
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self::quick()
    }
}

/// Структурированный результат неразрушающей проверки.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifyReport {
    pub deep: bool,
    pub level_count: usize,
    pub live_sstables: usize,
    pub entries_checked: u64,
    pub oldest_active_snapshot: Option<SequenceNumber>,
    pub inventory: FileInventory,
    pub warnings: Vec<String>,
}

pub(crate) fn verify_version(
    version: &VersionSet,
    identity: DatabaseId,
    limits: &Limits,
    max_file_size: u64,
    inventory: FileInventory,
    oldest_active_snapshot: Option<SequenceNumber>,
    options: VerifyOptions,
) -> Result<VerifyReport> {
    version.validate(limits)?;
    if !inventory.blockers.is_empty() {
        return Err(Error::corruption(
            "file inventory",
            inventory.blockers.join("; "),
        ));
    }

    let files = version.files_in_read_order()?;
    let mut entries_checked = 0_u64;
    if options.deep {
        for table in &files {
            let entries = table_scan::read_entries(
                table.path(),
                identity,
                table.metadata(),
                limits,
                max_file_size,
                options.verify_checksums,
            )?;
            entries_checked = entries_checked
                .checked_add(
                    u64::try_from(entries.len()).map_err(|_| Error::ResourceLimit {
                        resource: "verify_entries_checked",
                        limit: u64::MAX,
                        observed: u64::MAX,
                    })?,
                )
                .ok_or(Error::ResourceLimit {
                    resource: "verify_entries_checked",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })?;
        }
    }

    let mut warnings = Vec::new();
    append_warning(&mut warnings, "orphan SSTable", inventory.orphan_sst.len());
    append_warning(&mut warnings, "temporary object", inventory.temporary.len());
    append_warning(&mut warnings, "obsolete file", inventory.obsolete.len());
    append_warning(&mut warnings, "unknown file", inventory.unknown.len());

    Ok(VerifyReport {
        deep: options.deep,
        level_count: version.level_count(),
        live_sstables: files.len(),
        entries_checked,
        oldest_active_snapshot,
        inventory,
        warnings,
    })
}

fn append_warning(warnings: &mut Vec<String>, category: &str, count: usize) {
    if count != 0 {
        warnings.push(format!("{category}: {count}"));
    }
}
