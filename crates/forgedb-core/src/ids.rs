use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Error, Result};

static DATABASE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Стабильный 128-битный идентификатор базы, хранимый в `IDENTITY`.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DatabaseId([u8; 16]);

impl DatabaseId {
    /// Создаёт version-4-shaped идентификатор из независимо seeded hashers стандартной библиотеки.
    ///
    /// Идентификатор обнаруживает случайное смешивание файлов и не является криптографической защитой.
    #[must_use]
    pub fn random() -> Self {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let counter = DATABASE_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = process::id();

        let first_state = RandomState::new();
        let mut first = first_state.build_hasher();
        elapsed.as_nanos().hash(&mut first);
        counter.hash(&mut first);
        pid.hash(&mut first);
        let first_half = first.finish();

        let second_state = RandomState::new();
        let mut second = second_state.build_hasher();
        first_half.hash(&mut second);
        elapsed.as_secs().hash(&mut second);
        counter.rotate_left(17).hash(&mut second);
        let second_half = second.finish();

        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&first_half.to_be_bytes());
        bytes[8..].copy_from_slice(&second_half.to_be_bytes());
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(bytes)
    }

    /// Создаёт identity из исходных 16 байт.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Возвращает исходные байты identity.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Разбирает canonical идентификатор с дефисами.
    pub fn parse(input: &str) -> Result<Self> {
        if input.len() != 36 {
            return Err(Error::corruption(
                "database identity",
                "идентификатор должен содержать ровно 36 ASCII-байт",
            ));
        }

        let mut compact = [0_u8; 32];
        let mut compact_len = 0_usize;
        for (index, byte) in input.bytes().enumerate() {
            let separator = matches!(index, 8 | 13 | 18 | 23);
            if separator {
                if byte != b'-' {
                    return Err(Error::corruption(
                        "database identity",
                        "разделители идентификатора расположены неверно",
                    ));
                }
                continue;
            }
            if byte == b'-' || compact_len >= compact.len() {
                return Err(Error::corruption(
                    "database identity",
                    "идентификатор содержит неожиданный разделитель",
                ));
            }
            compact[compact_len] = byte;
            compact_len += 1;
        }

        if compact_len != compact.len() {
            return Err(Error::corruption(
                "database identity",
                "идентификатор содержит неверное число hexadecimal digits",
            ));
        }

        let mut bytes = [0_u8; 16];
        let (pairs, _) = compact.as_chunks::<2>();
        for (target, pair) in bytes.iter_mut().zip(pairs) {
            let high = decode_hex(pair[0])?;
            let low = decode_hex(pair[1])?;
            *target = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

fn decode_hex(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::corruption(
            "database identity",
            "идентификатор содержит не-hexadecimal байт",
        )),
    }
}

impl fmt::Display for DatabaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                formatter.write_str("-")?;
            }
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for DatabaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DatabaseId({self})")
    }
}

/// Идентификатор column family, встроенный в каждый internal key.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ColumnFamilyId(u32);

impl ColumnFamilyId {
    /// Default и единственная поддерживаемая column family ранних milestone.
    pub const DEFAULT: Self = Self(0);

    /// Создаёт идентификатор column family.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Возвращает underlying числовой идентификатор.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Глобальный ненулевой MVCC sequence number.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SequenceNumber(u64);

impl SequenceNumber {
    /// Первый допустимый sequence number.
    pub const FIRST: Self = Self(1);

    /// Создаёт sequence number и отклоняет зарезервированный ноль.
    pub fn new(value: u64) -> Result<Self> {
        if value == 0 {
            return Err(Error::invalid_argument(
                "sequence_number",
                "ноль зарезервирован",
            ));
        }
        Ok(Self(value))
    }

    /// Возвращает underlying число.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Возвращает следующий sequence number без wraparound.
    pub fn checked_next(self) -> Result<Self> {
        self.checked_add(1)
    }

    /// Добавляет положительный offset без wraparound и появления нуля.
    pub fn checked_add(self, offset: u64) -> Result<Self> {
        let value = self.0.checked_add(offset).ok_or(Error::ResourceLimit {
            resource: "sequence_number",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        Self::new(value)
    }

    /// Возвращает последний включённый sequence непустого диапазона mutations.
    pub fn inclusive_end(self, operation_count: usize) -> Result<Self> {
        if operation_count == 0 {
            return Err(Error::invalid_argument(
                "operation_count",
                "sequence range должен содержать хотя бы одну операцию",
            ));
        }
        let offset = u64::try_from(operation_count - 1).map_err(|_| Error::ResourceLimit {
            resource: "sequence_range",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        self.checked_add(offset)
    }
}

/// Тип записи internal key.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ValueType {
    /// Tombstone удаления.
    Deletion = 0,
    /// Сохранённое значение.
    Value = 1,
}

impl TryFrom<u8> for ValueType {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Deletion),
            1 => Ok(Self::Value),
            _ => Err(Error::corruption("internal key", "неизвестный value type")),
        }
    }
}

impl From<ValueType> for u8 {
    fn from(value: ValueType) -> Self {
        value as Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_id_round_trips() -> Result<()> {
        let id = DatabaseId::random();
        let encoded = id.to_string();
        let decoded = DatabaseId::parse(&encoded)?;
        assert_eq!(decoded, id);
        Ok(())
    }

    #[test]
    fn sequence_rejects_zero_and_overflow() -> Result<()> {
        assert!(SequenceNumber::new(0).is_err());
        let maximum = SequenceNumber::new(u64::MAX)?;
        assert!(maximum.checked_next().is_err());
        assert_eq!(SequenceNumber::FIRST.inclusive_end(3)?.get(), 3);
        Ok(())
    }
}
