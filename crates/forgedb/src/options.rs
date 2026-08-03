use forgedb_core::{Error, Limits, Result};

/// ForgeDB configuration validated before persisted files are opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options {
    /// Create a missing database directory in future `Database::open` implementations.
    pub create_if_missing: bool,
    /// Reject opening an already existing database.
    pub error_if_exists: bool,
    /// Open without permitting writes.
    pub read_only: bool,
    /// Maximum user-key bytes.
    pub max_key_size: usize,
    /// Maximum value bytes.
    pub max_value_size: usize,
    /// Maximum encoded write-batch bytes.
    pub max_batch_size: usize,
    /// Maximum operations in one write batch.
    pub max_batch_operations: usize,
    /// Mutable memtable flush threshold.
    pub memtable_size: usize,
    /// Target WAL file size.
    pub wal_file_size: u64,
    /// Target SSTable size.
    pub target_sst_size: u64,
    /// Target uncompressed data-block size.
    pub data_block_size: usize,
    /// Prefix-compression restart interval.
    pub restart_interval: usize,
    /// Bloom-filter bits per key.
    pub bloom_bits_per_key: u16,
    /// Maximum resident block-cache bytes.
    pub block_cache_size: usize,
    /// Maximum cached table readers.
    pub table_cache_capacity: usize,
    /// Number of LSM levels.
    pub level_count: usize,
    /// L0 file count that schedules compaction.
    pub l0_compaction_trigger: usize,
    /// L0 file count that slows writes.
    pub l0_slowdown_trigger: usize,
    /// L0 file count that stops writes.
    pub l0_stop_trigger: usize,
    /// Target size of level 1.
    pub level1_target_size: u64,
    /// Size multiplier between levels.
    pub level_size_multiplier: u64,
    /// Maximum concurrent background flush jobs.
    pub max_background_flushes: usize,
    /// Maximum concurrent background compaction jobs.
    pub max_background_compactions: usize,
    /// Maximum bytes selected by one compaction.
    pub max_compaction_bytes: u64,
    /// Verify checksums during normal reads.
    pub verify_checksums: bool,
    /// Enable additional invariant checks.
    pub paranoid_checks: bool,
}

impl Options {
    /// Validates every option and cross-field invariant without opening files.
    pub fn validate(&self) -> Result<()> {
        self.limits()?.validate()?;
        require_non_zero_usize("memtable_size", self.memtable_size)?;
        require_non_zero_u64("wal_file_size", self.wal_file_size)?;
        require_non_zero_u64("target_sst_size", self.target_sst_size)?;
        require_range_usize("data_block_size", self.data_block_size, 1024, 16 * 1024 * 1024)?;
        require_non_zero_usize("restart_interval", self.restart_interval)?;
        if !(1..=64).contains(&self.bloom_bits_per_key) {
            return Err(Error::invalid_argument(
                "bloom_bits_per_key",
                "must be in 1..=64",
            ));
        }
        require_non_zero_usize("table_cache_capacity", self.table_cache_capacity)?;
        require_range_usize("level_count", self.level_count, 2, 64)?;
        require_non_zero_usize("l0_compaction_trigger", self.l0_compaction_trigger)?;
        if self.l0_slowdown_trigger < self.l0_compaction_trigger {
            return Err(Error::invalid_argument(
                "l0_slowdown_trigger",
                "must be at least l0_compaction_trigger",
            ));
        }
        if self.l0_stop_trigger < self.l0_slowdown_trigger {
            return Err(Error::invalid_argument(
                "l0_stop_trigger",
                "must be at least l0_slowdown_trigger",
            ));
        }
        require_non_zero_u64("level1_target_size", self.level1_target_size)?;
        if self.level_size_multiplier < 2 {
            return Err(Error::invalid_argument(
                "level_size_multiplier",
                "must be at least 2",
            ));
        }
        require_non_zero_usize("max_background_flushes", self.max_background_flushes)?;
        require_non_zero_usize(
            "max_background_compactions",
            self.max_background_compactions,
        )?;
        if self.max_compaction_bytes < self.target_sst_size {
            return Err(Error::invalid_argument(
                "max_compaction_bytes",
                "must be at least target_sst_size",
            ));
        }

        let mut level_target = self.level1_target_size;
        for _ in 1..self.level_count {
            level_target = level_target
                .checked_mul(self.level_size_multiplier)
                .ok_or_else(|| {
                    Error::invalid_argument(
                        "level_size_multiplier",
                        "configured level targets overflow u64",
                    )
                })?;
        }
        Ok(())
    }

    /// Returns the bounded core limits after validation.
    pub fn limits(&self) -> Result<Limits> {
        Limits::new(
            self.max_key_size,
            self.max_value_size,
            self.max_batch_size,
            self.max_batch_operations,
        )
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            create_if_missing: true,
            error_if_exists: false,
            read_only: false,
            max_key_size: 1024 * 1024,
            max_value_size: 64 * 1024 * 1024,
            max_batch_size: 128 * 1024 * 1024,
            max_batch_operations: 100_000,
            memtable_size: 64 * 1024 * 1024,
            wal_file_size: 64 * 1024 * 1024,
            target_sst_size: 64 * 1024 * 1024,
            data_block_size: 16 * 1024,
            restart_interval: 16,
            bloom_bits_per_key: 10,
            block_cache_size: 256 * 1024 * 1024,
            table_cache_capacity: 512,
            level_count: 7,
            l0_compaction_trigger: 4,
            l0_slowdown_trigger: 8,
            l0_stop_trigger: 12,
            level1_target_size: 256 * 1024 * 1024,
            level_size_multiplier: 10,
            max_background_flushes: 1,
            max_background_compactions: 1,
            max_compaction_bytes: 512 * 1024 * 1024,
            verify_checksums: true,
            paranoid_checks: false,
        }
    }
}

fn require_non_zero_usize(field: &'static str, value: usize) -> Result<()> {
    if value == 0 {
        return Err(Error::invalid_argument(field, "must be greater than zero"));
    }
    Ok(())
}

fn require_non_zero_u64(field: &'static str, value: u64) -> Result<()> {
    if value == 0 {
        return Err(Error::invalid_argument(field, "must be greater than zero"));
    }
    Ok(())
}

fn require_range_usize(field: &'static str, value: usize, minimum: usize, maximum: usize) -> Result<()> {
    if !(minimum..=maximum).contains(&value) {
        return Err(Error::invalid_argument(
            field,
            format!("must be in {minimum}..={maximum}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() -> Result<()> {
        Options::default().validate()
    }

    #[test]
    fn invalid_l0_threshold_order_is_rejected() {
        let options = Options {
            l0_slowdown_trigger: 3,
            ..Options::default()
        };
        assert!(options.validate().is_err());
    }

    #[test]
    fn overflowing_level_targets_are_rejected() {
        let options = Options {
            level_count: 64,
            level1_target_size: u64::MAX / 2,
            level_size_multiplier: 10,
            ..Options::default()
        };
        assert!(options.validate().is_err());
    }
}
