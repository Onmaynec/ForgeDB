//! Журнал предзаписи ForgeDB: формат WAL V1, строгий replay и append-only writer.

mod checksum;
mod codec;
mod log;

pub use codec::{WAL_FRAME_HEADER_SIZE, WAL_HEADER_SIZE, WalBatch, WalCodec, WalHeader};
pub use log::{ReplaySummary, WAL_FILE_NAME, WAL_FILE_NUMBER, WalWriter, open_wal, replay_wal};
