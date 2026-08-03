use forgedb_core::{
    ColumnFamilyId, InternalKey, InternalKeyCodec, Limits, Result, SequenceNumber, ValueType,
};

#[derive(Clone, Copy)]
struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
}

#[test]
fn deterministic_round_trip_and_ordering_properties() -> Result<()> {
    let limits = Limits::new(64, 1024, 8192, 256)?;
    let mut random = XorShift64(0x5eed_f0db_2026_0001);
    let mut keys = Vec::new();

    for _ in 0..2_000 {
        let length = usize::try_from((random.next() % 32) + 1).map_err(|_| {
            forgedb_core::Error::InternalInvariant {
                invariant: "test_length_conversion",
                reason: "generated length did not fit usize".to_owned(),
            }
        })?;
        let mut user_key = Vec::with_capacity(length);
        for _ in 0..length {
            user_key.push(random.next().to_le_bytes()[0]);
        }
        let sequence = SequenceNumber::new((random.next() % 10_000) + 1)?;
        let value_type = if random.next() & 1 == 0 {
            ValueType::Deletion
        } else {
            ValueType::Value
        };
        let key = InternalKey::new(
            ColumnFamilyId::new((random.next() % 3) as u32),
            user_key,
            sequence,
            value_type,
            &limits,
        )?;
        let encoded = InternalKeyCodec::encode(&key, &limits)?;
        assert_eq!(InternalKeyCodec::decode(&encoded, &limits)?, key);
        keys.push(key);
    }

    keys.sort();
    for adjacent in keys.windows(2) {
        let left = &adjacent[0];
        let right = &adjacent[1];
        assert!(left <= right);
        if left.column_family() == right.column_family() && left.user_key() == right.user_key() {
            assert!(left.sequence() >= right.sequence());
            if left.sequence() == right.sequence() {
                assert!(left.value_type() >= right.value_type());
            }
        }
    }
    Ok(())
}
