use crate::{ColumnFamilyId, Error, Limits, Result};

/// One operation in a write batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BatchOperation {
    /// Store or replace one value.
    Put {
        /// Target column family.
        column_family: ColumnFamilyId,
        /// Binary user key.
        key: Vec<u8>,
        /// Binary value.
        value: Vec<u8>,
    },
    /// Add a deletion tombstone.
    Delete {
        /// Target column family.
        column_family: ColumnFamilyId,
        /// Binary user key.
        key: Vec<u8>,
    },
}

/// Atomically constructed collection of mutations.
///
/// The batch mutates only after an operation and its resulting aggregate size have passed all
/// configured limits.
#[derive(Clone, Debug)]
pub struct WriteBatch {
    operations: Vec<BatchOperation>,
    encoded_size: usize,
    limits: Limits,
}

impl WriteBatch {
    /// Creates a batch with default limits.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    /// Creates a batch with caller-provided limits.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            operations: Vec::new(),
            encoded_size: 0,
            limits,
        }
    }

    /// Adds a put in the default column family.
    pub fn put(&mut self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<()> {
        self.put_cf(ColumnFamilyId::DEFAULT, key, value)
    }

    /// Adds a put in a column family.
    pub fn put_cf(
        &mut self,
        column_family: ColumnFamilyId,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
    ) -> Result<()> {
        ensure_supported_column_family(column_family)?;
        let key = key.as_ref();
        let value = value.as_ref();
        self.limits.validate_key(key)?;
        self.limits.validate_value(value)?;
        let operation_size = put_encoded_size(column_family, key.len(), value.len())?;
        self.reserve_operation(operation_size)?;
        self.operations.push(BatchOperation::Put {
            column_family,
            key: key.to_vec(),
            value: value.to_vec(),
        });
        self.encoded_size += operation_size;
        Ok(())
    }

    /// Adds a deletion in the default column family.
    pub fn delete(&mut self, key: impl AsRef<[u8]>) -> Result<()> {
        self.delete_cf(ColumnFamilyId::DEFAULT, key)
    }

    /// Adds a deletion in a column family.
    pub fn delete_cf(
        &mut self,
        column_family: ColumnFamilyId,
        key: impl AsRef<[u8]>,
    ) -> Result<()> {
        ensure_supported_column_family(column_family)?;
        let key = key.as_ref();
        self.limits.validate_key(key)?;
        let operation_size = delete_encoded_size(column_family, key.len())?;
        self.reserve_operation(operation_size)?;
        self.operations.push(BatchOperation::Delete {
            column_family,
            key: key.to_vec(),
        });
        self.encoded_size += operation_size;
        Ok(())
    }

    /// Returns queued operations.
    #[must_use]
    pub fn operations(&self) -> &[BatchOperation] {
        &self.operations
    }

    /// Returns the exact future WAL payload size for the current V1 operation encoding.
    #[must_use]
    pub const fn encoded_size(&self) -> usize {
        self.encoded_size
    }

    /// Returns the number of operations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Returns whether the batch contains no operations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Revalidates the complete batch against a different set of database limits.
    pub fn validate_against(&self, limits: &Limits) -> Result<()> {
        limits.validate()?;
        if self.operations.len() > limits.max_batch_operations {
            return Err(resource_limit(
                "batch_operations",
                limits.max_batch_operations,
                self.operations.len(),
            ));
        }
        let mut total = 0_usize;
        for operation in &self.operations {
            let operation_size = match operation {
                BatchOperation::Put {
                    column_family,
                    key,
                    value,
                } => {
                    ensure_supported_column_family(*column_family)?;
                    limits.validate_key(key)?;
                    limits.validate_value(value)?;
                    put_encoded_size(*column_family, key.len(), value.len())?
                }
                BatchOperation::Delete { column_family, key } => {
                    ensure_supported_column_family(*column_family)?;
                    limits.validate_key(key)?;
                    delete_encoded_size(*column_family, key.len())?
                }
            };
            total = total.checked_add(operation_size).ok_or(Error::ResourceLimit {
                resource: "batch_encoded_size",
                limit: u64::try_from(limits.max_batch_size).unwrap_or(u64::MAX),
                observed: u64::MAX,
            })?;
            if total > limits.max_batch_size {
                return Err(resource_limit(
                    "batch_encoded_size",
                    limits.max_batch_size,
                    total,
                ));
            }
        }
        Ok(())
    }

    fn reserve_operation(&self, operation_size: usize) -> Result<()> {
        let next_count = self.operations.len().checked_add(1).ok_or(Error::ResourceLimit {
            resource: "batch_operations",
            limit: u64::try_from(self.limits.max_batch_operations).unwrap_or(u64::MAX),
            observed: u64::MAX,
        })?;
        if next_count > self.limits.max_batch_operations {
            return Err(resource_limit(
                "batch_operations",
                self.limits.max_batch_operations,
                next_count,
            ));
        }
        let next_size = self
            .encoded_size
            .checked_add(operation_size)
            .ok_or(Error::ResourceLimit {
                resource: "batch_encoded_size",
                limit: u64::try_from(self.limits.max_batch_size).unwrap_or(u64::MAX),
                observed: u64::MAX,
            })?;
        if next_size > self.limits.max_batch_size {
            return Err(resource_limit(
                "batch_encoded_size",
                self.limits.max_batch_size,
                next_size,
            ));
        }
        Ok(())
    }
}

impl Default for WriteBatch {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_supported_column_family(column_family: ColumnFamilyId) -> Result<()> {
    if column_family != ColumnFamilyId::DEFAULT {
        return Err(Error::invalid_argument(
            "column_family",
            "only the default column family is supported before v0.7.0",
        ));
    }
    Ok(())
}

fn put_encoded_size(
    column_family: ColumnFamilyId,
    key_length: usize,
    value_length: usize,
) -> Result<usize> {
    checked_sum(&[
        1,
        varint32_size(column_family.get()),
        varint_length_size(key_length)?,
        varint_length_size(value_length)?,
        key_length,
        value_length,
    ])
}

fn delete_encoded_size(column_family: ColumnFamilyId, key_length: usize) -> Result<usize> {
    checked_sum(&[
        1,
        varint32_size(column_family.get()),
        varint_length_size(key_length)?,
        1,
        key_length,
    ])
}

fn varint_length_size(value: usize) -> Result<usize> {
    let value = u32::try_from(value).map_err(|_| Error::ResourceLimit {
        resource: "batch_length_field",
        limit: u64::from(u32::MAX),
        observed: u64::try_from(value).unwrap_or(u64::MAX),
    })?;
    Ok(varint32_size(value))
}

const fn varint32_size(value: u32) -> usize {
    match value {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0x0fff_ffff => 4,
        _ => 5,
    }
}

fn checked_sum(values: &[usize]) -> Result<usize> {
    let mut total = 0_usize;
    for value in values {
        total = total.checked_add(*value).ok_or(Error::ResourceLimit {
            resource: "batch_encoded_size",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    }
    Ok(total)
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
    fn rejected_operation_does_not_mutate_batch() -> Result<()> {
        let limits = Limits::new(4, 4, 32, 1)?;
        let mut batch = WriteBatch::with_limits(limits);
        batch.put(b"key", b"v")?;
        let before_size = batch.encoded_size();
        assert!(batch.delete(b"next").is_err());
        assert_eq!(batch.len(), 1);
        assert_eq!(batch.encoded_size(), before_size);
        Ok(())
    }

    #[test]
    fn exact_size_accounting_matches_expected_layout() -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.put(b"abc", b"xy")?;
        assert_eq!(batch.encoded_size(), 1 + 1 + 1 + 1 + 3 + 2);
        batch.delete(b"z")?;
        assert_eq!(batch.encoded_size(), 9 + 1 + 1 + 1 + 1 + 1);
        Ok(())
    }
}
