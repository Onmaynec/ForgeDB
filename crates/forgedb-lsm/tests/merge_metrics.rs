use forgedb_core::{ColumnFamilyId, InternalKey, Limits, Result, SequenceNumber, ValueType};
use forgedb_lsm::{
    CompactionCursor, CompactionMerge, CompactionMergeOptions, VecCompactionCursor,
};
use forgedb_sstable::TableEntry;

#[test]
fn bottommost_tombstone_drop_is_counted() -> Result<()> {
    let entries = vec![
        table_entry(5, ValueType::Deletion, b"")?,
        table_entry(3, ValueType::Value, b"old")?,
    ];
    let sources: Vec<Box<dyn CompactionCursor>> =
        vec![Box::new(VecCompactionCursor::new(entries))];
    let mut merge = CompactionMerge::new(
        sources,
        CompactionMergeOptions {
            bottommost: true,
            ..CompactionMergeOptions::default()
        },
    )?;

    let output = merge.by_ref().collect::<Result<Vec<_>>>()?;
    assert!(output.is_empty());
    assert_eq!(merge.stats().input_entries, 2);
    assert_eq!(merge.stats().output_entries, 0);
    assert_eq!(merge.stats().dropped_versions, 2);
    assert_eq!(merge.stats().dropped_tombstones, 1);
    Ok(())
}

fn table_entry(sequence: u64, value_type: ValueType, value: &[u8]) -> Result<TableEntry> {
    Ok(TableEntry {
        internal_key: InternalKey::new(
            ColumnFamilyId::DEFAULT,
            b"key",
            SequenceNumber::new(sequence)?,
            value_type,
            &Limits::default(),
        )?,
        value: value.to_vec(),
    })
}
