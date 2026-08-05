//! MANIFEST V1 ForgeDB: authoritative VersionEdit log, CURRENT и L0 metadata.

mod bootstrap;
mod checksum;
mod codec;
mod log;
mod state;

pub const CURRENT_FILE_NAME: &str = "CURRENT";
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
pub use log::{ManifestWriter, OpenManifest, ReplaySummary, open_manifest, replay_manifest};
pub use state::{ManifestState, VersionEdit};
