//! Журнал предзаписи ForgeDB: формат WAL V1, строгий replay и numbered generations.

mod checksum;
mod codec;
mod log;

pub use codec::{WAL_FRAME_HEADER_SIZE, WAL_HEADER_SIZE, WalBatch, WalCodec, WalHeader};
pub use log::{
    ReplaySummary, WAL_FILE_NAME, WAL_FILE_NUMBER, WalWriter, create_wal, open_wal,
    open_wal_numbered, parse_wal_file_name, replay_wal, replay_wal_numbered, wal_file_name,
};
