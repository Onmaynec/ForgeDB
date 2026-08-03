use std::cmp::Ordering;

use crate::{ColumnFamilyId, Error, Limits, Result, SequenceNumber, ValueType};

const MAGIC: &[u8; 4] = b"FIKY";
const FORMAT_VERSION: u8 = 1;
const FIXED_PREFIX_SIZE: usize = 16;
const FIXED_SUFFIX_SIZE: usize = 9;

/// One MVCC internal key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InternalKey {
    column_family: ColumnFamilyId,
    user_key: Vec<u8>,
    sequence: SequenceNumber,
    value_type: ValueType,
}

impl InternalKey {
    /// Creates a bounded internal key.
    pub fn new(
        column_family: ColumnFamilyId,
        user_key: impl Into<Vec<u8>>,
        sequence: SequenceNumber,
        value_type: ValueType,
        limits: &Limits,
    ) -> Result<Self> {
        let user_key = user_key.into();
        limits.validate_key(&user_key)?;
        Ok(Self {
            column_family,
            user_key,
            sequence,
            value_type,
        })
    }

    /// Returns the column-family identifier.
    #[must_use]
    pub const fn column_family(&self) -> ColumnFamilyId {
        self.column_family
    }

    /// Returns the user-key bytes.
    #[must_use]
    pub fn user_key(&self) -> &[u8] {
        &self.user_key
    }

    /// Returns the sequence number.
    #[must_use]
    pub const fn sequence(&self) -> SequenceNumber {
        self.sequence
    }

    /// Returns the value type.
    #[must_use]
    pub const fn value_type(&self) -> ValueType {
        self.value_type
    }
}

impl Ord for InternalKey {
    fn cmp(&self, other: &Self) -> Ordering {
        InternalKeyComparator.compare(self, other)
    }
}

impl PartialOrd for InternalKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Logical internal-key comparator.
#[derive(Clone, Copy, Debug, Default)]
pub struct InternalKeyComparator;

impl InternalKeyComparator {
    /// Stable comparator identity persisted by later MANIFEST versions.
    pub const NAME: &'static str = "forgedb.internal-key.bytewise.v1";

    /// Compares by column family ascending, user key ascending, sequence descending, then type
    /// descending.
    #[must_use]
    pub fn compare(self, left: &InternalKey, right: &InternalKey) -> Ordering {
        left.column_family
            .cmp(&right.column_family)
            .then_with(|| left.user_key.cmp(&right.user_key))
            .then_with(|| right.sequence.cmp(&left.sequence))
            .then_with(|| right.value_type.cmp(&left.value_type))
    }
}

/// Bounded Internal Key V1 encoder and decoder.
#[derive(Clone, Copy, Debug, Default)]
pub struct InternalKeyCodec;

impl InternalKeyCodec {
    /// Current supported format version.
    pub const VERSION: u8 = FORMAT_VERSION;

    /// Encodes an internal key using the canonical V1 layout.
    pub fn encode(key: &InternalKey, limits: &Limits) -> Result<Vec<u8>> {
        limits.validate_key(key.user_key())?;
        let total = FIXED_PREFIX_SIZE
            .checked_add(key.user_key.len())
            .and_then(|value| value.checked_add(FIXED_SUFFIX_SIZE))
            .ok_or(Error::ResourceLimit {
                resource: "internal_key_encoded_size",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        let mut encoded = Vec::with_capacity(total);
        encoded.extend_from_slice(MAGIC);
        encoded.push(FORMAT_VERSION);
        encoded.extend_from_slice(&[0_u8; 3]);
        encoded.extend_from_slice(&key.column_family.get().to_le_bytes());
        let key_length = u32::try_from(key.user_key.len()).map_err(|_| Error::ResourceLimit {
            resource: "internal_key_user_key_length",
            limit: u64::from(u32::MAX),
            observed: u64::try_from(key.user_key.len()).unwrap_or(u64::MAX),
        })?;
        encoded.extend_from_slice(&key_length.to_le_bytes());
        encoded.extend_from_slice(&key.user_key);
        encoded.extend_from_slice(&key.sequence.get().to_le_bytes());
        encoded.push(key.value_type.into());
        Ok(encoded)
    }

    /// Decodes exactly one Internal Key V1 value.
    pub fn decode(encoded: &[u8], limits: &Limits) -> Result<InternalKey> {
        if encoded.len() < FIXED_PREFIX_SIZE + FIXED_SUFFIX_SIZE {
            return Err(Error::corruption("internal key", "input is truncated"));
        }
        if encoded.get(..4) != Some(MAGIC.as_slice()) {
            return Err(Error::corruption("internal key", "magic mismatch"));
        }
        let version = *encoded
            .get(4)
            .ok_or_else(|| Error::corruption("internal key", "missing version"))?;
        if version != FORMAT_VERSION {
            return Err(Error::UnsupportedFormat {
                component: "internal key",
                found: u32::from(version),
                minimum_supported: u32::from(FORMAT_VERSION),
                maximum_supported: u32::from(FORMAT_VERSION),
            });
        }
        if encoded.get(5..8) != Some([0_u8; 3].as_slice()) {
            return Err(Error::corruption(
                "internal key",
                "reserved header bytes are non-zero",
            ));
        }

        let column_family = read_u32(encoded, 8, "column family")?;
        let key_length_u32 = read_u32(encoded, 12, "user-key length")?;
        let key_length = usize::try_from(key_length_u32).map_err(|_| Error::ResourceLimit {
            resource: "internal_key_user_key_length",
            limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
            observed: u64::from(key_length_u32),
        })?;
        if key_length == 0 || key_length > limits.max_key_size {
            return Err(Error::ResourceLimit {
                resource: "internal_key_user_key_length",
                limit: u64::try_from(limits.max_key_size).unwrap_or(u64::MAX),
                observed: u64::try_from(key_length).unwrap_or(u64::MAX),
            });
        }

        let key_start = FIXED_PREFIX_SIZE;
        let key_end = key_start
            .checked_add(key_length)
            .ok_or_else(|| Error::corruption("internal key", "user-key range overflows"))?;
        let expected_length = key_end
            .checked_add(FIXED_SUFFIX_SIZE)
            .ok_or_else(|| Error::corruption("internal key", "encoded length overflows"))?;
        if encoded.len() != expected_length {
            return Err(Error::corruption(
                "internal key",
                "encoded length does not match user-key length",
            ));
        }

        let user_key = encoded
            .get(key_start..key_end)
            .ok_or_else(|| Error::corruption("internal key", "user key is truncated"))?
            .to_vec();
        let sequence = read_u64(encoded, key_end, "sequence")?;
        let value_type_offset = key_end
            .checked_add(8)
            .ok_or_else(|| Error::corruption("internal key", "value-type offset overflows"))?;
        let value_type = ValueType::try_from(*encoded.get(value_type_offset).ok_or_else(|| {
            Error::corruption("internal key", "value type is truncated")
        })?)?;

        InternalKey::new(
            ColumnFamilyId::new(column_family),
            user_key,
            SequenceNumber::new(sequence).map_err(|_| {
                Error::corruption("internal key", "sequence zero is reserved")
            })?,
            value_type,
            limits,
        )
    }
}

fn read_u32(input: &[u8], offset: usize, field: &'static str) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| Error::corruption("internal key", format!("{field} offset overflows")))?;
    let bytes: [u8; 4] = input
        .get(offset..end)
        .ok_or_else(|| Error::corruption("internal key", format!("{field} is truncated")))?
        .try_into()
        .map_err(|_| Error::corruption("internal key", format!("{field} has wrong width")))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &[u8], offset: usize, field: &'static str) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| Error::corruption("internal key", format!("{field} offset overflows")))?;
    let bytes: [u8; 8] = input
        .get(offset..end)
        .ok_or_else(|| Error::corruption("internal key", format!("{field} is truncated")))?
        .try_into()
        .map_err(|_| Error::corruption("internal key", format!("{field} has wrong width")))?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &[u8], sequence: u64, value_type: ValueType) -> Result<InternalKey> {
        InternalKey::new(
            ColumnFamilyId::DEFAULT,
            name.to_vec(),
            SequenceNumber::new(sequence)?,
            value_type,
            &Limits::default(),
        )
    }

    #[test]
    fn round_trip_binary_key() -> Result<()> {
        let original = key(&[0, 1, 255, 0], 42, ValueType::Value)?;
        let encoded = InternalKeyCodec::encode(&original, &Limits::default())?;
        let decoded = InternalKeyCodec::decode(&encoded, &Limits::default())?;
        assert_eq!(decoded, original);
        Ok(())
    }

    #[test]
    fn sorts_newest_sequence_first() -> Result<()> {
        let newer = key(b"same", 9, ValueType::Value)?;
        let older = key(b"same", 2, ValueType::Value)?;
        assert!(newer < older);
        Ok(())
    }

    #[test]
    fn rejects_future_version_and_trailing_bytes() -> Result<()> {
        let original = key(b"key", 1, ValueType::Value)?;
        let mut encoded = InternalKeyCodec::encode(&original, &Limits::default())?;
        encoded[4] = 2;
        assert!(matches!(
            InternalKeyCodec::decode(&encoded, &Limits::default()),
            Err(Error::UnsupportedFormat { .. })
        ));

        encoded[4] = 1;
        encoded.push(0);
        assert!(InternalKeyCodec::decode(&encoded, &Limits::default()).is_err());
        Ok(())
    }

    #[test]
    fn rejects_oversized_length_without_allocating() -> Result<()> {
        let original = key(b"key", 1, ValueType::Value)?;
        let mut encoded = InternalKeyCodec::encode(&original, &Limits::default())?;
        encoded[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(InternalKeyCodec::decode(&encoded, &Limits::default()).is_err());
        Ok(())
    }
}
