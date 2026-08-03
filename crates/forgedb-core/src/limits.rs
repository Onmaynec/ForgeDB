use crate::{Error, Result};

/// Bounds applied to user keys, values, and write batches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum user-key size in bytes.
    pub max_key_size: usize,
    /// Maximum value size in bytes.
    pub max_value_size: usize,
    /// Maximum encoded write-batch payload size in bytes.
    pub max_batch_size: usize,
    /// Maximum number of operations in one batch.
    pub max_batch_operations: usize,
}

impl Limits {
    /// Creates and validates limits.
    pub fn new(
        max_key_size: usize,
        max_value_size: usize,
        max_batch_size: usize,
        max_batch_operations: usize,
    ) -> Result<Self> {
        let limits = Self {
            max_key_size,
            max_value_size,
            max_batch_size,
            max_batch_operations,
        };
        limits.validate()?;
        Ok(limits)
    }

    /// Validates all individual and cross-field limits.
    pub fn validate(&self) -> Result<()> {
        if self.max_key_size == 0 {
            return Err(Error::invalid_argument(
                "max_key_size",
                "must be greater than zero",
            ));
        }
        if self.max_value_size == 0 {
            return Err(Error::invalid_argument(
                "max_value_size",
                "must be greater than zero",
            ));
        }
        if self.max_batch_size < self.max_value_size {
            return Err(Error::invalid_argument(
                "max_batch_size",
                "must be at least max_value_size",
            ));
        }
        if self.max_batch_operations == 0 {
            return Err(Error::invalid_argument(
                "max_batch_operations",
                "must be greater than zero",
            ));
        }
        Ok(())
    }

    /// Validates a user key.
    pub fn validate_key(&self, key: &[u8]) -> Result<()> {
        if key.is_empty() {
            return Err(Error::invalid_argument("key", "must not be empty"));
        }
        if key.len() > self.max_key_size {
            return Err(resource_limit("key_size", self.max_key_size, key.len()));
        }
        Ok(())
    }

    /// Validates a value.
    pub fn validate_value(&self, value: &[u8]) -> Result<()> {
        if value.len() > self.max_value_size {
            return Err(resource_limit(
                "value_size",
                self.max_value_size,
                value.len(),
            ));
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_key_size: 1024 * 1024,
            max_value_size: 64 * 1024 * 1024,
            max_batch_size: 128 * 1024 * 1024,
            max_batch_operations: 100_000,
        }
    }
}

fn resource_limit(resource: &'static str, limit: usize, observed: usize) -> Error {
    Error::ResourceLimit {
        resource,
        limit: u64::try_from(limit).unwrap_or(u64::MAX),
        observed: u64::try_from(observed).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_boundaries() -> Result<()> {
        let limits = Limits::new(3, 4, 16, 2)?;
        limits.validate_key(b"abc")?;
        limits.validate_value(b"1234")?;
        assert!(limits.validate_key(b"").is_err());
        assert!(limits.validate_key(b"abcd").is_err());
        assert!(limits.validate_value(b"12345").is_err());
        Ok(())
    }
}
