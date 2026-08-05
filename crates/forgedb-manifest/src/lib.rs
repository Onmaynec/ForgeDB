//! Versioned MANIFEST contracts ForgeDB.
//!
//! MANIFEST V1 остаётся читаемым. MANIFEST V2 определяет level-aware
//! persisted contract, отдельный writer/replay, crash-safe migration snapshot
//! и strict format dispatch. Database integration выполняется только под
//! exclusive database lock и после recovery tests.

mod bootstrap;
mod checksum;
mod codec;
#[cfg(test)]
mod compatibility;
mod dispatch;
mod log;
mod state;
mod v2;
mod v2_log;

pub const CURRENT_FILE_NAME: &str = "CURRENT";
/// Версия initial bootstrap writer. Existing database может открыть V1 или V2.
pub const MANIFEST_FORMAT_VERSION: u16 = 1;
pub const MANIFEST_HEADER_SIZE: usize = 64;
pub const MANIFEST_FRAME_HEADER_SIZE: usize = 32;
pub const INITIAL_MANIFEST_NUMBER: u64 = 1;
pub const INITIAL_LOG_NUMBER: u64 = 1;
pub const MINIMUM_READER_VERSION: u32 = 1;

/// Canonical имя MANIFEST.
#[must_use]
pub fn manifest_file_name(number: u64) -> String {
    format!("MANIFEST-{number:06}")
}

/// Разбирает canonical имя MANIFEST.
#[must_use]
pub fn parse_manifest_file_name(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("MANIFEST-")?;
    if digits.len() < 6 || digits.bytes().any(|byte| !byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub use bootstrap::create_manifest;
pub use dispatch::{
    OpenManifestAny, bridge_manifest_v1_to_v2, next_available_manifest_number,
    open_manifest_any,
};
pub use log::{ManifestWriter, OpenManifest, ReplaySummary, open_manifest, replay_manifest};
pub use state::{ManifestState, VersionEdit};
pub use v2::{
    CompactionPointer, DeletedLevelFile, LevelFile, MANIFEST_V2_FORMAT_VERSION,
    MANIFEST_V2_MAX_LEVELS, MANIFEST_V2_MINIMUM_READER_VERSION, ManifestStateV2, ManifestV2Codec,
    ManifestV2Header, VersionEditV2,
};
pub use v2_log::{
    ManifestV2Writer, OpenManifestV2, migrate_manifest_v1_to_v2, migrated_v2_state,
    open_manifest_v2, replay_manifest_v2,
};
