use forgedb_core::{Error, Result};

const MAGIC: &[u8; 4] = b"FBLM";
const VERSION: u16 = 1;
const HASH_ID_FNV1A_DOUBLE: u8 = 1;
const HEADER_SIZE: usize = 20;
const MAX_HASH_COUNT: u8 = 30;

#[derive(Clone, Debug)]
pub(crate) struct BloomFilter {
    hash_count: u8,
    bit_count: u32,
    key_count: u64,
    bits: Vec<u8>,
}

impl BloomFilter {
    pub(crate) fn build<'a, I>(keys: I, bits_per_key: u16) -> Result<Self>
    where
        I: IntoIterator<Item = &'a [u8]>,
    {
        if !(1..=64).contains(&bits_per_key) {
            return Err(Error::invalid_argument(
                "bloom_bits_per_key",
                "значение должно находиться в диапазоне 1..=64",
            ));
        }
        let keys: Vec<&[u8]> = keys.into_iter().collect();
        let key_count = u64::try_from(keys.len()).map_err(|_| Error::ResourceLimit {
            resource: "bloom_key_count",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
        let requested_bits = key_count
            .checked_mul(u64::from(bits_per_key))
            .ok_or(Error::ResourceLimit {
                resource: "bloom_bit_count",
                limit: u64::from(u32::MAX),
                observed: u64::MAX,
            })?;
        let bit_count_u64 = requested_bits.max(64).min(u64::from(u32::MAX));
        let byte_count_u64 = bit_count_u64
            .checked_add(7)
            .ok_or(Error::ResourceLimit {
                resource: "bloom_byte_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?
            / 8;
        let byte_count = usize::try_from(byte_count_u64).map_err(|_| Error::ResourceLimit {
            resource: "bloom_byte_count",
            limit: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
            observed: byte_count_u64,
        })?;
        let bit_count = u32::try_from(byte_count_u64.saturating_mul(8)).map_err(|_| {
            Error::ResourceLimit {
                resource: "bloom_bit_count",
                limit: u64::from(u32::MAX),
                observed: byte_count_u64.saturating_mul(8),
            }
        })?;
        let calculated = ((u32::from(bits_per_key) * 69) / 100).clamp(1, u32::from(MAX_HASH_COUNT));
        let hash_count = u8::try_from(calculated).map_err(|_| Error::InternalInvariant {
            invariant: "I-BLOOM-HASH-COUNT",
            reason: "рассчитанное число hash-функций не помещается в u8".to_owned(),
        })?;
        let mut filter = Self {
            hash_count,
            bit_count,
            key_count,
            bits: vec![0_u8; byte_count],
        };
        for key in keys {
            filter.insert(key)?;
        }
        Ok(filter)
    }

    pub(crate) fn may_contain(&self, key: &[u8]) -> bool {
        if self.key_count == 0 {
            return false;
        }
        let (first, second) = hash_pair(key);
        let modulo = u64::from(self.bit_count);
        for index in 0..self.hash_count {
            let bit = first
                .wrapping_add(u64::from(index).wrapping_mul(second))
                % modulo;
            let byte_index = usize::try_from(bit / 8).unwrap_or(usize::MAX);
            let mask = 1_u8 << u32::try_from(bit % 8).unwrap_or(0);
            if self
                .bits
                .get(byte_index)
                .is_none_or(|stored| stored & mask == 0)
            {
                return false;
            }
        }
        true
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(HEADER_SIZE + self.bits.len());
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&VERSION.to_le_bytes());
        output.push(HASH_ID_FNV1A_DOUBLE);
        output.push(self.hash_count);
        output.extend_from_slice(&self.bit_count.to_le_bytes());
        output.extend_from_slice(&self.key_count.to_le_bytes());
        output.extend_from_slice(&self.bits);
        output
    }

    pub(crate) fn decode(input: &[u8], max_bytes: usize) -> Result<Self> {
        if input.len() < HEADER_SIZE {
            return Err(Error::corruption("SSTable Bloom filter", "заголовок усечён"));
        }
        if input.len() > max_bytes {
            return Err(Error::ResourceLimit {
                resource: "sstable_filter_bytes",
                limit: u64::try_from(max_bytes).unwrap_or(u64::MAX),
                observed: u64::try_from(input.len()).unwrap_or(u64::MAX),
            });
        }
        if input.get(0..4) != Some(MAGIC.as_slice()) {
            return Err(Error::corruption("SSTable Bloom filter", "неверная сигнатура"));
        }
        let version = read_u16(input, 4)?;
        if version != VERSION {
            return Err(Error::UnsupportedFormat {
                component: "SSTable Bloom filter",
                found: u32::from(version),
                minimum_supported: u32::from(VERSION),
                maximum_supported: u32::from(VERSION),
            });
        }
        if input.get(6).copied() != Some(HASH_ID_FNV1A_DOUBLE) {
            return Err(Error::UnsupportedFormat {
                component: "SSTable Bloom hash",
                found: u32::from(input.get(6).copied().unwrap_or(0)),
                minimum_supported: u32::from(HASH_ID_FNV1A_DOUBLE),
                maximum_supported: u32::from(HASH_ID_FNV1A_DOUBLE),
            });
        }
        let hash_count = input
            .get(7)
            .copied()
            .ok_or_else(|| Error::corruption("SSTable Bloom filter", "hash_count отсутствует"))?;
        if hash_count == 0 || hash_count > MAX_HASH_COUNT {
            return Err(Error::corruption(
                "SSTable Bloom filter",
                "hash_count выходит за допустимые границы",
            ));
        }
        let bit_count = read_u32(input, 8)?;
        if bit_count < 64 || bit_count % 8 != 0 {
            return Err(Error::corruption(
                "SSTable Bloom filter",
                "bit_count должен быть кратен 8 и не меньше 64",
            ));
        }
        let bytes = usize::try_from(bit_count / 8).map_err(|_| Error::ResourceLimit {
            resource: "sstable_filter_bytes",
            limit: u64::try_from(max_bytes).unwrap_or(u64::MAX),
            observed: u64::from(bit_count / 8),
        })?;
        let expected = HEADER_SIZE
            .checked_add(bytes)
            .ok_or_else(|| Error::corruption("SSTable Bloom filter", "размер переполнен"))?;
        if input.len() != expected {
            return Err(Error::corruption(
                "SSTable Bloom filter",
                "длина bitset не совпадает с bit_count",
            ));
        }
        Ok(Self {
            hash_count,
            bit_count,
            key_count: read_u64(input, 12)?,
            bits: input[HEADER_SIZE..].to_vec(),
        })
    }

    fn insert(&mut self, key: &[u8]) -> Result<()> {
        let (first, second) = hash_pair(key);
        let modulo = u64::from(self.bit_count);
        for index in 0..self.hash_count {
            let bit = first
                .wrapping_add(u64::from(index).wrapping_mul(second))
                % modulo;
            let byte_index = usize::try_from(bit / 8).map_err(|_| Error::InternalInvariant {
                invariant: "I-BLOOM-BIT-INDEX",
                reason: "индекс Bloom filter не помещается в usize".to_owned(),
            })?;
            let shift = u32::try_from(bit % 8).map_err(|_| Error::InternalInvariant {
                invariant: "I-BLOOM-BIT-SHIFT",
                reason: "смещение Bloom filter не помещается в u32".to_owned(),
            })?;
            let byte = self.bits.get_mut(byte_index).ok_or(Error::InternalInvariant {
                invariant: "I-BLOOM-BIT-BOUNDS",
                reason: "рассчитанный бит находится вне bitset".to_owned(),
            })?;
            *byte |= 1_u8 << shift;
        }
        Ok(())
    }
}

fn hash_pair(input: &[u8]) -> (u64, u64) {
    let first = fnv1a64(input, 0xcbf2_9ce4_8422_2325);
    let second = fnv1a64(input, 0x8422_2325_cbf2_9ce4) | 1;
    (first, second)
}

fn fnv1a64(input: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for byte in input {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn read_u16(input: &[u8], offset: usize) -> Result<u16> {
    let bytes: [u8; 2] = input
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| Error::corruption("SSTable Bloom filter", "поле u16 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption("SSTable Bloom filter", "поле u16 имеет неверную ширину"))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(input: &[u8], offset: usize) -> Result<u32> {
    let bytes: [u8; 4] = input
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| Error::corruption("SSTable Bloom filter", "поле u32 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption("SSTable Bloom filter", "поле u32 имеет неверную ширину"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &[u8], offset: usize) -> Result<u64> {
    let bytes: [u8; 8] = input
        .get(offset..offset.saturating_add(8))
        .ok_or_else(|| Error::corruption("SSTable Bloom filter", "поле u64 усечено"))?
        .try_into()
        .map_err(|_| Error::corruption("SSTable Bloom filter", "поле u64 имеет неверную ширину"))?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::BloomFilter;
    use forgedb_core::Result;

    #[test]
    fn round_trip_has_no_false_negatives() -> Result<()> {
        let keys: [&[u8]; 4] = [b"alpha", b"beta", b"gamma", b"delta"];
        let filter = BloomFilter::build(keys, 10)?;
        let encoded = filter.encode();
        let decoded = BloomFilter::decode(&encoded, 1024)?;
        for key in keys {
            assert!(decoded.may_contain(key));
        }
        Ok(())
    }
}
