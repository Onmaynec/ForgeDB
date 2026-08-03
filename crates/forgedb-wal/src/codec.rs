use std::time::{SystemTime, UNIX_EPOCH};

use forgedb_core::{
    BatchOperation, ColumnFamilyId, DatabaseId, Error, Limits, Result, SequenceNumber, ValueType,
    WriteBatch,
};

use crate::checksum::crc32c;

pub const WAL_HEADER_SIZE: usize = 64;
pub const WAL_FRAME_HEADER_SIZE: usize = 32;
const WAL_MAGIC: &[u8; 8] = b"FORGEWAL";
const FRAME_MAGIC: &[u8; 4] = b"FWF1";
const WAL_VERSION: u16 = 1;
const WRITE_BATCH_RECORD: u8 = 1;

/// Заголовок файла WAL V1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WalHeader {
    pub database_id: DatabaseId,
    pub file_number: u64,
    pub created_unix_ms: u64,
}

impl WalHeader {
    /// Создаёт заголовок нового WAL.
    #[must_use]
    pub fn new(database_id: DatabaseId, file_number: u64) -> Self {
        let created_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            database_id,
            file_number,
            created_unix_ms: u64::try_from(created_unix_ms).unwrap_or(u64::MAX),
        }
    }

    /// Кодирует заголовок в фиксированные 64 байта.
    #[must_use]
    pub fn encode(self) -> [u8; WAL_HEADER_SIZE] {
        let mut output = [0_u8; WAL_HEADER_SIZE];
        output[0..8].copy_from_slice(WAL_MAGIC);
        output[8..10].copy_from_slice(&WAL_VERSION.to_le_bytes());
        output[10..12].copy_from_slice(&(WAL_HEADER_SIZE as u16).to_le_bytes());
        output[12..16].copy_from_slice(&0_u32.to_le_bytes());
        output[16..32].copy_from_slice(self.database_id.as_bytes());
        output[32..40].copy_from_slice(&self.file_number.to_le_bytes());
        output[40..48].copy_from_slice(&self.created_unix_ms.to_le_bytes());
        let checksum = crc32c(&output[..60]);
        output[60..64].copy_from_slice(&checksum.to_le_bytes());
        output
    }

    /// Декодирует и полностью проверяет заголовок WAL V1.
    pub fn decode(
        input: &[u8; WAL_HEADER_SIZE],
        expected_database_id: DatabaseId,
        expected_file_number: u64,
    ) -> Result<Self> {
        if &input[0..8] != WAL_MAGIC {
            return Err(Error::corruption("WAL header", "неверная сигнатура"));
        }
        let version = read_u16(input, 8, "format_version")?;
        if version != WAL_VERSION {
            return Err(Error::UnsupportedFormat {
                component: "WAL",
                found: u32::from(version),
                minimum_supported: u32::from(WAL_VERSION),
                maximum_supported: u32::from(WAL_VERSION),
            });
        }
        let header_length = read_u16(input, 10, "header_length")?;
        if usize::from(header_length) != WAL_HEADER_SIZE {
            return Err(Error::corruption("WAL header", "неверная длина заголовка"));
        }
        if read_u32(input, 12, "flags")? != 0 || input[48..60].iter().any(|byte| *byte != 0) {
            return Err(Error::corruption(
                "WAL header",
                "зарезервированные поля должны быть нулевыми",
            ));
        }
        let stored_checksum = read_u32(input, 60, "checksum")?;
        if crc32c(&input[..60]) != stored_checksum {
            return Err(Error::corruption("WAL header", "CRC32C не совпадает"));
        }
        let mut database_bytes = [0_u8; 16];
        database_bytes.copy_from_slice(&input[16..32]);
        let database_id = DatabaseId::from_bytes(database_bytes);
        if database_id != expected_database_id {
            return Err(Error::corruption(
                "WAL header",
                "идентификатор базы данных не совпадает",
            ));
        }
        let file_number = read_u64(input, 32, "file_number")?;
        if file_number != expected_file_number {
            return Err(Error::corruption(
                "WAL header",
                "номер WAL не совпадает с ожидаемым",
            ));
        }
        Ok(Self {
            database_id,
            file_number,
            created_unix_ms: read_u64(input, 40, "created_unix_ms")?,
        })
    }
}

/// Полностью декодированный атомарный batch из WAL.
#[derive(Clone, Debug)]
pub struct WalBatch {
    pub sequence_start: SequenceNumber,
    pub batch: WriteBatch,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameHeader {
    pub total_length: usize,
    pub payload_length: usize,
    pub operation_count: usize,
    pub sequence_start: SequenceNumber,
    pub stored_checksum: u32,
}

/// Кодек кадров WAL V1.
#[derive(Clone, Copy, Debug, Default)]
pub struct WalCodec;

impl WalCodec {
    /// Кодирует один WriteBatch в один логический кадр WAL V1.
    pub fn encode_frame(
        sequence_start: SequenceNumber,
        batch: &WriteBatch,
        limits: &Limits,
    ) -> Result<Vec<u8>> {
        batch.validate_against(limits)?;
        if batch.is_empty() {
            return Err(Error::invalid_argument("batch", "пустой batch нельзя записать в WAL"));
        }
        sequence_start.inclusive_end(batch.len())?;

        let mut payload = Vec::with_capacity(batch.encoded_size());
        for operation in batch.operations() {
            match operation {
                BatchOperation::Put {
                    column_family,
                    key,
                    value,
                } => {
                    payload.push(u8::from(ValueType::Value));
                    encode_varint32(column_family.get(), &mut payload);
                    encode_length(key.len(), &mut payload)?;
                    encode_length(value.len(), &mut payload)?;
                    payload.extend_from_slice(key);
                    payload.extend_from_slice(value);
                }
                BatchOperation::Delete { column_family, key } => {
                    payload.push(u8::from(ValueType::Deletion));
                    encode_varint32(column_family.get(), &mut payload);
                    encode_length(key.len(), &mut payload)?;
                    encode_varint32(0, &mut payload);
                    payload.extend_from_slice(key);
                }
            }
        }
        if payload.len() != batch.encoded_size() {
            return Err(Error::InternalInvariant {
                invariant: "I-WAL-PAYLOAD-SIZE",
                reason: "расчётный размер WriteBatch не совпал с кодированным payload".to_owned(),
            });
        }

        let payload_length = u32::try_from(payload.len()).map_err(|_| Error::ResourceLimit {
            resource: "wal_payload_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(payload.len()).unwrap_or(u64::MAX),
        })?;
        let total_length_usize = WAL_FRAME_HEADER_SIZE
            .checked_add(payload.len())
            .ok_or(Error::ResourceLimit {
                resource: "wal_frame_length",
                limit: u64::from(u32::MAX),
                observed: u64::MAX,
            })?;
        let total_length = u32::try_from(total_length_usize).map_err(|_| Error::ResourceLimit {
            resource: "wal_frame_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(total_length_usize).unwrap_or(u64::MAX),
        })?;
        let operation_count = u32::try_from(batch.len()).map_err(|_| Error::ResourceLimit {
            resource: "wal_operation_count",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(batch.len()).unwrap_or(u64::MAX),
        })?;

        let mut frame = vec![0_u8; WAL_FRAME_HEADER_SIZE];
        frame[0..4].copy_from_slice(FRAME_MAGIC);
        frame[4..8].copy_from_slice(&total_length.to_le_bytes());
        frame[12] = WRITE_BATCH_RECORD;
        frame[13] = 0;
        frame[14..16].copy_from_slice(&0_u16.to_le_bytes());
        frame[16..24].copy_from_slice(&sequence_start.get().to_le_bytes());
        frame[24..28].copy_from_slice(&operation_count.to_le_bytes());
        frame[28..32].copy_from_slice(&payload_length.to_le_bytes());
        frame.extend_from_slice(&payload);

        let checksum = crc32c(&frame[12..]);
        frame[8..12].copy_from_slice(&checksum.to_le_bytes());
        Ok(frame)
    }

    /// Проверяет фиксированную часть кадра до выделения памяти под payload.
    pub(crate) fn decode_frame_header(
        input: &[u8; WAL_FRAME_HEADER_SIZE],
        limits: &Limits,
    ) -> Result<FrameHeader> {
        if &input[0..4] != FRAME_MAGIC {
            return Err(Error::corruption("WAL frame", "неверная сигнатура кадра"));
        }
        if input[12] != WRITE_BATCH_RECORD {
            return Err(Error::corruption("WAL frame", "неизвестный тип записи"));
        }
        if input[13] != 0 || read_u16(input, 14, "reserved")? != 0 {
            return Err(Error::corruption(
                "WAL frame",
                "flags и reserved должны быть нулевыми",
            ));
        }
        let total_u32 = read_u32(input, 4, "total_frame_length")?;
        let payload_u32 = read_u32(input, 28, "payload_length")?;
        let total_length = usize::try_from(total_u32).map_err(|_| Error::ResourceLimit {
            resource: "wal_frame_length",
            limit: u64::from(u32::MAX),
            observed: u64::from(total_u32),
        })?;
        let payload_length = usize::try_from(payload_u32).map_err(|_| Error::ResourceLimit {
            resource: "wal_payload_length",
            limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
            observed: u64::from(payload_u32),
        })?;
        if payload_length > limits.max_batch_size {
            return Err(Error::ResourceLimit {
                resource: "wal_payload_length",
                limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
                observed: u64::try_from(payload_length).unwrap_or(u64::MAX),
            });
        }
        let expected_total = WAL_FRAME_HEADER_SIZE
            .checked_add(payload_length)
            .ok_or_else(|| Error::corruption("WAL frame", "длина кадра переполнена"))?;
        if total_length != expected_total {
            return Err(Error::corruption(
                "WAL frame",
                "total_frame_length не совпадает с payload_length",
            ));
        }
        let operation_u32 = read_u32(input, 24, "operation_count")?;
        let operation_count = usize::try_from(operation_u32).map_err(|_| Error::ResourceLimit {
            resource: "wal_operation_count",
            limit: u64::try_from(limits.max_batch_operations).unwrap_or(u64::MAX),
            observed: u64::from(operation_u32),
        })?;
        if operation_count == 0 || operation_count > limits.max_batch_operations {
            return Err(Error::ResourceLimit {
                resource: "wal_operation_count",
                limit: u64::try_from(limits.max_batch_operations).unwrap_or(u64::MAX),
                observed: u64::try_from(operation_count).unwrap_or(u64::MAX),
            });
        }
        let sequence_start = SequenceNumber::new(read_u64(input, 16, "sequence_start")?)
            .map_err(|_| Error::corruption("WAL frame", "sequence_start равен нулю"))?;
        sequence_start.inclusive_end(operation_count)?;
        Ok(FrameHeader {
            total_length,
            payload_length,
            operation_count,
            sequence_start,
            stored_checksum: read_u32(input, 8, "frame_crc32c")?,
        })
    }

    /// Декодирует полный кадр; частичный кадр сюда передавать нельзя.
    pub(crate) fn decode_complete_frame(
        header_bytes: &[u8; WAL_FRAME_HEADER_SIZE],
        payload: &[u8],
        limits: &Limits,
    ) -> Result<WalBatch> {
        let header = Self::decode_frame_header(header_bytes, limits)?;
        if payload.len() != header.payload_length {
            return Err(Error::corruption("WAL frame", "payload физически усечён"));
        }
        let mut checksum_input = Vec::with_capacity(20 + payload.len());
        checksum_input.extend_from_slice(&header_bytes[12..32]);
        checksum_input.extend_from_slice(payload);
        if crc32c(&checksum_input) != header.stored_checksum {
            return Err(Error::corruption("WAL frame", "CRC32C кадра не совпадает"));
        }

        let mut batch = WriteBatch::with_limits(*limits);
        let mut cursor = 0_usize;
        for _ in 0..header.operation_count {
            let operation_type = read_byte(payload, &mut cursor, "operation_type")?;
            let column_family = ColumnFamilyId::new(decode_varint32(payload, &mut cursor)?);
            let key_length = decode_length(payload, &mut cursor, limits.max_key_size, "key_length")?;
            let value_length =
                decode_length(payload, &mut cursor, limits.max_value_size, "value_length")?;
            let key = take_bytes(payload, &mut cursor, key_length, "key")?;
            match ValueType::try_from(operation_type) {
                Ok(ValueType::Value) => {
                    let value = take_bytes(payload, &mut cursor, value_length, "value")?;
                    batch.put_cf(column_family, key, value)?;
                }
                Ok(ValueType::Deletion) => {
                    if value_length != 0 {
                        return Err(Error::corruption(
                            "WAL payload",
                            "delete содержит ненулевую длину значения",
                        ));
                    }
                    batch.delete_cf(column_family, key)?;
                }
                Err(_) => {
                    return Err(Error::corruption(
                        "WAL payload",
                        "неизвестный тип операции",
                    ));
                }
            }
        }
        if cursor != payload.len() {
            return Err(Error::corruption(
                "WAL payload",
                "после последней операции остались лишние байты",
            ));
        }
        if batch.len() != header.operation_count {
            return Err(Error::corruption(
                "WAL payload",
                "operation_count не совпадает с декодированным batch",
            ));
        }
        Ok(WalBatch {
            sequence_start: header.sequence_start,
            batch,
        })
    }
}

fn encode_length(value: usize, output: &mut Vec<u8>) -> Result<()> {
    let value = u32::try_from(value).map_err(|_| Error::ResourceLimit {
        resource: "wal_length_field",
        limit: u64::from(u32::MAX),
        observed: u64::try_from(value).unwrap_or(u64::MAX),
    })?;
    encode_varint32(value, output);
    Ok(())
}

fn encode_varint32(mut value: u32, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn decode_length(
    input: &[u8],
    cursor: &mut usize,
    maximum: usize,
    field: &'static str,
) -> Result<usize> {
    let value = decode_varint32(input, cursor)?;
    let value = usize::try_from(value).map_err(|_| Error::ResourceLimit {
        resource: field,
        limit: u64::try_from(maximum).unwrap_or(u64::MAX),
        observed: u64::MAX,
    })?;
    if value > maximum {
        return Err(Error::ResourceLimit {
            resource: field,
            limit: u64::try_from(maximum).unwrap_or(u64::MAX),
            observed: u64::try_from(value).unwrap_or(u64::MAX),
        });
    }
    Ok(value)
}

fn decode_varint32(input: &[u8], cursor: &mut usize) -> Result<u32> {
    let mut value = 0_u32;
    for shift in [0_u32, 7, 14, 21, 28] {
        let byte = read_byte(input, cursor, "varint32")?;
        if shift == 28 && byte > 0x0f {
            return Err(Error::corruption("WAL payload", "varint32 переполнен"));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Error::corruption("WAL payload", "varint32 слишком длинный"))
}

fn read_byte(input: &[u8], cursor: &mut usize, field: &'static str) -> Result<u8> {
    let byte = *input
        .get(*cursor)
        .ok_or_else(|| Error::corruption("WAL payload", format!("поле {field} усечено")))?;
    *cursor = cursor
        .checked_add(1)
        .ok_or_else(|| Error::corruption("WAL payload", "смещение переполнено"))?;
    Ok(byte)
}

fn take_bytes<'a>(
    input: &'a [u8],
    cursor: &mut usize,
    length: usize,
    field: &'static str,
) -> Result<&'a [u8]> {
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| Error::corruption("WAL payload", format!("диапазон {field} переполнен")))?;
    let bytes = input
        .get(*cursor..end)
        .ok_or_else(|| Error::corruption("WAL payload", format!("поле {field} усечено")))?;
    *cursor = end;
    Ok(bytes)
}

fn read_u16(input: &[u8], offset: usize, field: &'static str) -> Result<u16> {
    let bytes: [u8; 2] = input
        .get(offset..offset + 2)
        .ok_or_else(|| Error::corruption("WAL", format!("поле {field} усечено")))?
        .try_into()
        .map_err(|_| Error::corruption("WAL", format!("неверная ширина поля {field}")))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(input: &[u8], offset: usize, field: &'static str) -> Result<u32> {
    let bytes: [u8; 4] = input
        .get(offset..offset + 4)
        .ok_or_else(|| Error::corruption("WAL", format!("поле {field} усечено")))?
        .try_into()
        .map_err(|_| Error::corruption("WAL", format!("неверная ширина поля {field}")))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &[u8], offset: usize, field: &'static str) -> Result<u64> {
    let bytes: [u8; 8] = input
        .get(offset..offset + 8)
        .ok_or_else(|| Error::corruption("WAL", format!("поле {field} усечено")))?
        .try_into()
        .map_err(|_| Error::corruption("WAL", format!("неверная ширина поля {field}")))?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip_and_future_version_rejection() -> Result<()> {
        let id = DatabaseId::random();
        let header = WalHeader::new(id, 7);
        let encoded = header.encode();
        assert_eq!(WalHeader::decode(&encoded, id, 7)?, header);

        let mut future = encoded;
        future[8..10].copy_from_slice(&2_u16.to_le_bytes());
        let checksum = crc32c(&future[..60]);
        future[60..64].copy_from_slice(&checksum.to_le_bytes());
        assert!(matches!(
            WalHeader::decode(&future, id, 7),
            Err(Error::UnsupportedFormat { found: 2, .. })
        ));
        Ok(())
    }

    #[test]
    fn frame_round_trip_is_exact() -> Result<()> {
        let limits = Limits::default();
        let mut batch = WriteBatch::new();
        batch.put(b"alpha", b"one")?;
        batch.delete(b"beta")?;
        let frame = WalCodec::encode_frame(SequenceNumber::FIRST, &batch, &limits)?;
        let header: [u8; WAL_FRAME_HEADER_SIZE] = frame[..WAL_FRAME_HEADER_SIZE]
            .try_into()
            .map_err(|_| Error::corruption("test", "неверная длина заголовка"))?;
        let decoded = WalCodec::decode_complete_frame(
            &header,
            &frame[WAL_FRAME_HEADER_SIZE..],
            &limits,
        )?;
        assert_eq!(decoded.sequence_start, SequenceNumber::FIRST);
        assert_eq!(decoded.batch.operations(), batch.operations());
        Ok(())
    }
}
