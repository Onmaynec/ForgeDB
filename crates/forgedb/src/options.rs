use forgedb_core::{Error, Limits, Result};

/// Конфигурация ForgeDB, проверяемая до открытия persisted-файлов.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options {
    /// Создавать отсутствующий каталог базы при read-write открытии.
    pub create_if_missing: bool,
    /// Отклонять открытие уже существующей базы.
    pub error_if_exists: bool,
    /// Открывать базу без разрешения операций записи.
    pub read_only: bool,
    /// Максимальный размер user key в байтах.
    pub max_key_size: usize,
    /// Максимальный размер value в байтах.
    pub max_value_size: usize,
    /// Максимальный размер закодированного WriteBatch в байтах.
    pub max_batch_size: usize,
    /// Максимальное число операций в одном WriteBatch.
    pub max_batch_operations: usize,
    /// Максимальный объём mutable memtable до отклонения новых записей.
    pub memtable_size: usize,
    /// Целевой размер WAL-файла для будущей rotation policy.
    pub wal_file_size: u64,
    /// Целевой размер SSTable.
    pub target_sst_size: u64,
    /// Целевой размер несжатого data block.
    pub data_block_size: usize,
    /// Интервал restart points для prefix compression.
    pub restart_interval: usize,
    /// Число бит Bloom filter на один ключ.
    pub bloom_bits_per_key: u16,
    /// Максимальный объём resident block cache в байтах.
    pub block_cache_size: usize,
    /// Максимальное число cached table readers.
    pub table_cache_capacity: usize,
    /// Число уровней LSM.
    pub level_count: usize,
    /// Число L0-файлов, запускающее compaction.
    pub l0_compaction_trigger: usize,
    /// Число L0-файлов, замедляющее записи.
    pub l0_slowdown_trigger: usize,
    /// Число L0-файлов, останавливающее записи.
    pub l0_stop_trigger: usize,
    /// Целевой размер уровня 1.
    pub level1_target_size: u64,
    /// Множитель размера между уровнями.
    pub level_size_multiplier: u64,
    /// Максимальное число одновременных фоновых flush jobs.
    pub max_background_flushes: usize,
    /// Максимальное число одновременных фоновых compaction jobs.
    pub max_background_compactions: usize,
    /// Максимальный объём данных одной compaction.
    pub max_compaction_bytes: u64,
    /// Проверять checksum при обычном чтении.
    pub verify_checksums: bool,
    /// Включать дополнительные проверки инвариантов.
    pub paranoid_checks: bool,
}

impl Options {
    /// Проверяет каждую опцию и межполевые инварианты без открытия файлов.
    pub fn validate(&self) -> Result<()> {
        self.limits()?.validate()?;
        require_non_zero_usize("memtable_size", self.memtable_size)?;
        require_non_zero_u64("wal_file_size", self.wal_file_size)?;
        require_non_zero_u64("target_sst_size", self.target_sst_size)?;
        require_range_usize(
            "data_block_size",
            self.data_block_size,
            1024,
            16 * 1024 * 1024,
        )?;
        require_non_zero_usize("restart_interval", self.restart_interval)?;
        if !(1..=64).contains(&self.bloom_bits_per_key) {
            return Err(Error::invalid_argument(
                "bloom_bits_per_key",
                "значение должно находиться в диапазоне 1..=64",
            ));
        }
        require_non_zero_usize("table_cache_capacity", self.table_cache_capacity)?;
        require_range_usize("level_count", self.level_count, 2, 64)?;
        require_non_zero_usize("l0_compaction_trigger", self.l0_compaction_trigger)?;
        if self.l0_slowdown_trigger < self.l0_compaction_trigger {
            return Err(Error::invalid_argument(
                "l0_slowdown_trigger",
                "значение должно быть не меньше l0_compaction_trigger",
            ));
        }
        if self.l0_stop_trigger < self.l0_slowdown_trigger {
            return Err(Error::invalid_argument(
                "l0_stop_trigger",
                "значение должно быть не меньше l0_slowdown_trigger",
            ));
        }
        require_non_zero_u64("level1_target_size", self.level1_target_size)?;
        if self.level_size_multiplier < 2 {
            return Err(Error::invalid_argument(
                "level_size_multiplier",
                "значение должно быть не меньше 2",
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
                "значение должно быть не меньше target_sst_size",
            ));
        }

        let mut level_target = self.level1_target_size;
        for _ in 1..self.level_count {
            level_target = level_target
                .checked_mul(self.level_size_multiplier)
                .ok_or_else(|| {
                    Error::invalid_argument(
                        "level_size_multiplier",
                        "рассчитанные размеры уровней переполняют u64",
                    )
                })?;
        }
        Ok(())
    }

    /// Возвращает bounded core limits после проверки их согласованности.
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
        return Err(Error::invalid_argument(field, "значение должно быть больше нуля"));
    }
    Ok(())
}

fn require_non_zero_u64(field: &'static str, value: u64) -> Result<()> {
    if value == 0 {
        return Err(Error::invalid_argument(field, "значение должно быть больше нуля"));
    }
    Ok(())
}

fn require_range_usize(
    field: &'static str,
    value: usize,
    minimum: usize,
    maximum: usize,
) -> Result<()> {
    if !(minimum..=maximum).contains(&value) {
        return Err(Error::invalid_argument(
            field,
            format!("значение должно находиться в диапазоне {minimum}..={maximum}"),
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
