use std::collections::VecDeque;

use forgedb_core::{ColumnFamilyId, Error, InternalKey, Result};
use forgedb_sstable::TableEntry;

const ESTIMATED_ENTRY_OVERHEAD: u64 = 32;
const DEFAULT_TARGET_FILE_SIZE: u64 = 64 * 1024 * 1024;
const DEFAULT_MAX_KEY_GROUP_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_MAX_VERSIONS_PER_KEY: usize = 1_000_000;

/// Bounded policy потокового разбиения compaction output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompactionOutputOptions {
    /// Целевой приблизительный размер одного output SSTable.
    pub target_file_size: u64,
    /// Жёсткий лимит buffered bytes всех versions одного logical key.
    pub max_key_group_bytes: u64,
    /// Жёсткий лимит versions одного logical key.
    pub max_versions_per_user_key: usize,
}

impl Default for CompactionOutputOptions {
    fn default() -> Self {
        Self {
            target_file_size: DEFAULT_TARGET_FILE_SIZE,
            max_key_group_bytes: DEFAULT_MAX_KEY_GROUP_BYTES,
            max_versions_per_user_key: DEFAULT_MAX_VERSIONS_PER_KEY,
        }
    }
}

impl CompactionOutputOptions {
    pub fn validate(self) -> Result<()> {
        if self.target_file_size == 0 {
            return Err(Error::invalid_argument(
                "target_file_size",
                "целевой размер output SSTable должен быть больше нуля",
            ));
        }
        if self.max_key_group_bytes == 0 {
            return Err(Error::invalid_argument(
                "max_key_group_bytes",
                "лимит key-group должен быть больше нуля",
            ));
        }
        if self.max_versions_per_user_key == 0 {
            return Err(Error::invalid_argument(
                "max_versions_per_user_key",
                "лимит versions должен быть больше нуля",
            ));
        }
        Ok(())
    }
}

/// Завершённый логический output file до фактической SSTable serialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionOutputSummary {
    pub output_index: u64,
    pub entry_count: u64,
    pub estimated_bytes: u64,
    pub smallest_user_key: Vec<u8>,
    pub largest_user_key: Vec<u8>,
    /// Истина только когда один неделимый key-group сам превышает target.
    pub oversized_key_group: bool,
}

/// Поток событий для sink, создающего временные SSTable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompactionOutputEvent {
    BeginFile { output_index: u64 },
    Entry(TableEntry),
    FinishFile(CompactionOutputSummary),
}

/// Неперсистентные счётчики одного output split.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompactionOutputStats {
    pub input_entries: u64,
    pub estimated_input_bytes: u64,
    pub output_files: u64,
    pub oversized_key_groups: u64,
    pub peak_key_group_bytes: u64,
    pub peak_versions_per_user_key: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LogicalKey {
    column_family: ColumnFamilyId,
    user_key: Vec<u8>,
}

impl LogicalKey {
    fn from_entry(entry: &TableEntry) -> Self {
        Self {
            column_family: entry.internal_key.column_family(),
            user_key: entry.internal_key.user_key().to_vec(),
        }
    }

    fn matches(&self, entry: &TableEntry) -> bool {
        self.column_family == entry.internal_key.column_family()
            && self.user_key == entry.internal_key.user_key()
    }
}

#[derive(Debug)]
struct KeyGroup {
    entries: Vec<TableEntry>,
    estimated_bytes: u64,
}

#[derive(Debug)]
struct OpenOutput {
    output_index: u64,
    entry_count: u64,
    estimated_bytes: u64,
    smallest_user_key: Vec<u8>,
    largest_user_key: Vec<u8>,
}

/// Преобразует sorted retained entries в bounded поток output-file events.
///
/// Split выполняется только перед новым logical key. В памяти удерживаются
/// event queue и не более одной key-group, ограниченной options.
pub struct CompactionOutputSplitter<I>
where
    I: Iterator<Item = Result<TableEntry>>,
{
    source: I,
    lookahead: Option<TableEntry>,
    previous_internal_key: Option<InternalKey>,
    pending: VecDeque<CompactionOutputEvent>,
    open_output: Option<OpenOutput>,
    next_output_index: u64,
    options: CompactionOutputOptions,
    stats: CompactionOutputStats,
    source_exhausted: bool,
    failed: bool,
}

impl<I> CompactionOutputSplitter<I>
where
    I: Iterator<Item = Result<TableEntry>>,
{
    pub fn new(source: I, options: CompactionOutputOptions) -> Result<Self> {
        options.validate()?;
        Ok(Self {
            source,
            lookahead: None,
            previous_internal_key: None,
            pending: VecDeque::new(),
            open_output: None,
            next_output_index: 0,
            options,
            stats: CompactionOutputStats::default(),
            source_exhausted: false,
            failed: false,
        })
    }

    #[must_use]
    pub const fn stats(&self) -> CompactionOutputStats {
        self.stats
    }

    fn fill_pending(&mut self) -> Result<bool> {
        if self.source_exhausted {
            if self.open_output.is_some() {
                self.finish_open_output()?;
                return Ok(true);
            }
            return Ok(false);
        }

        let Some(group) = self.read_group()? else {
            self.source_exhausted = true;
            if self.open_output.is_some() {
                self.finish_open_output()?;
                return Ok(true);
            }
            return Ok(false);
        };

        let split_before_group = match &self.open_output {
            Some(open) => open
                .estimated_bytes
                .checked_add(group.estimated_bytes)
                .is_none_or(|combined| combined > self.options.target_file_size),
            None => false,
        };
        if split_before_group {
            self.finish_open_output()?;
        }
        if self.open_output.is_none() {
            self.begin_output()?;
        }
        self.append_group(group)?;
        Ok(true)
    }

    fn read_group(&mut self) -> Result<Option<KeyGroup>> {
        let Some(first) = self.pull_entry()? else {
            return Ok(None);
        };
        let logical_key = LogicalKey::from_entry(&first);
        let mut entries = vec![first];
        let mut estimated_bytes = estimated_entry_bytes(entries.first().ok_or(
            Error::InternalInvariant {
                invariant: "I-COMPACTION-OUTPUT-GROUP-FIRST",
                reason: "new key-group не содержит first entry".to_owned(),
            },
        )?)?;

        loop {
            let Some(entry) = self.pull_entry()? else {
                self.source_exhausted = true;
                break;
            };
            if !logical_key.matches(&entry) {
                self.lookahead = Some(entry);
                break;
            }
            let entry_bytes = estimated_entry_bytes(&entry)?;
            estimated_bytes = estimated_bytes.checked_add(entry_bytes).ok_or(
                Error::ResourceLimit {
                    resource: "compaction_key_group_bytes",
                    limit: self.options.max_key_group_bytes,
                    observed: u64::MAX,
                },
            )?;
            entries.push(entry);
            self.validate_group_limits(entries.len(), estimated_bytes)?;
        }
        self.validate_group_limits(entries.len(), estimated_bytes)?;
        self.stats.peak_key_group_bytes = self
            .stats
            .peak_key_group_bytes
            .max(estimated_bytes);
        self.stats.peak_versions_per_user_key = self
            .stats
            .peak_versions_per_user_key
            .max(entries.len());
        Ok(Some(KeyGroup {
            entries,
            estimated_bytes,
        }))
    }

    fn pull_entry(&mut self) -> Result<Option<TableEntry>> {
        let entry = match self.lookahead.take() {
            Some(entry) => Some(entry),
            None => self.source.next().transpose()?,
        };
        let Some(entry) = entry else {
            return Ok(None);
        };
        if self
            .previous_internal_key
            .as_ref()
            .is_some_and(|previous| previous >= &entry.internal_key)
        {
            return Err(Error::corruption(
                "compaction output ordering",
                "internal keys не строго возрастают",
            ));
        }
        self.previous_internal_key = Some(entry.internal_key.clone());
        self.stats.input_entries = checked_add(
            self.stats.input_entries,
            1,
            "compaction_output_input_entries",
        )?;
        self.stats.estimated_input_bytes = checked_add(
            self.stats.estimated_input_bytes,
            estimated_entry_bytes(&entry)?,
            "compaction_output_input_bytes",
        )?;
        Ok(Some(entry))
    }

    fn validate_group_limits(&self, versions: usize, estimated_bytes: u64) -> Result<()> {
        if versions > self.options.max_versions_per_user_key {
            return Err(Error::ResourceLimit {
                resource: "compaction_output_versions_per_user_key",
                limit: u64::try_from(self.options.max_versions_per_user_key)
                    .unwrap_or(u64::MAX),
                observed: u64::try_from(versions).unwrap_or(u64::MAX),
            });
        }
        if estimated_bytes > self.options.max_key_group_bytes {
            return Err(Error::ResourceLimit {
                resource: "compaction_key_group_bytes",
                limit: self.options.max_key_group_bytes,
                observed: estimated_bytes,
            });
        }
        Ok(())
    }

    fn begin_output(&mut self) -> Result<()> {
        let output_index = self.next_output_index;
        self.next_output_index = self
            .next_output_index
            .checked_add(1)
            .ok_or(Error::ResourceLimit {
                resource: "compaction_output_index",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
        self.open_output = Some(OpenOutput {
            output_index,
            entry_count: 0,
            estimated_bytes: 0,
            smallest_user_key: Vec::new(),
            largest_user_key: Vec::new(),
        });
        self.pending
            .push_back(CompactionOutputEvent::BeginFile { output_index });
        Ok(())
    }

    fn append_group(&mut self, group: KeyGroup) -> Result<()> {
        let first_key = group
            .entries
            .first()
            .ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-OUTPUT-GROUP-NONEMPTY",
                reason: "output group неожиданно пуста".to_owned(),
            })?
            .internal_key
            .user_key()
            .to_vec();
        let open = self
            .open_output
            .as_mut()
            .ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-OUTPUT-OPEN",
                reason: "append group выполняется без открытого output".to_owned(),
            })?;
        if open.entry_count == 0 {
            open.smallest_user_key = first_key.clone();
        }
        open.largest_user_key = first_key;
        open.entry_count = checked_add(
            open.entry_count,
            u64::try_from(group.entries.len()).map_err(|_| Error::ResourceLimit {
                resource: "compaction_output_entry_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?,
            "compaction_output_entry_count",
        )?;
        open.estimated_bytes = checked_add(
            open.estimated_bytes,
            group.estimated_bytes,
            "compaction_output_estimated_bytes",
        )?;
        for entry in group.entries {
            self.pending
                .push_back(CompactionOutputEvent::Entry(entry));
        }
        Ok(())
    }

    fn finish_open_output(&mut self) -> Result<()> {
        let open = self.open_output.take().ok_or(Error::InternalInvariant {
            invariant: "I-COMPACTION-OUTPUT-FINISH-OPEN",
            reason: "finish вызван без открытого output".to_owned(),
        })?;
        if open.entry_count == 0
            || open.smallest_user_key.is_empty()
            || open.largest_user_key.is_empty()
        {
            return Err(Error::InternalInvariant {
                invariant: "I-COMPACTION-OUTPUT-NONEMPTY",
                reason: "нельзя завершить пустой output file".to_owned(),
            });
        }
        let oversized_key_group = open.estimated_bytes > self.options.target_file_size;
        self.stats.output_files = checked_add(
            self.stats.output_files,
            1,
            "compaction_output_files",
        )?;
        if oversized_key_group {
            self.stats.oversized_key_groups = checked_add(
                self.stats.oversized_key_groups,
                1,
                "compaction_oversized_key_groups",
            )?;
        }
        self.pending
            .push_back(CompactionOutputEvent::FinishFile(
                CompactionOutputSummary {
                    output_index: open.output_index,
                    entry_count: open.entry_count,
                    estimated_bytes: open.estimated_bytes,
                    smallest_user_key: open.smallest_user_key,
                    largest_user_key: open.largest_user_key,
                    oversized_key_group,
                },
            ));
        Ok(())
    }
}

impl<I> Iterator for CompactionOutputSplitter<I>
where
    I: Iterator<Item = Result<TableEntry>>,
{
    type Item = Result<CompactionOutputEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            match self.fill_pending() {
                Ok(true) => {}
                Ok(false) => return None,
                Err(error) => {
                    self.failed = true;
                    return Some(Err(error));
                }
            }
        }
    }
}

fn estimated_entry_bytes(entry: &TableEntry) -> Result<u64> {
    let key_bytes = u64::try_from(entry.internal_key.user_key().len()).map_err(|_| {
        Error::ResourceLimit {
            resource: "compaction_output_user_key_bytes",
            limit: u64::MAX,
            observed: u64::MAX,
        }
    })?;
    let value_bytes = u64::try_from(entry.value.len()).map_err(|_| Error::ResourceLimit {
        resource: "compaction_output_value_bytes",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    key_bytes
        .checked_add(value_bytes)
        .and_then(|value| value.checked_add(ESTIMATED_ENTRY_OVERHEAD))
        .ok_or(Error::ResourceLimit {
            resource: "compaction_output_entry_bytes",
            limit: u64::MAX,
            observed: u64::MAX,
        })
}

fn checked_add(left: u64, right: u64, resource: &'static str) -> Result<u64> {
    left.checked_add(right).ok_or(Error::ResourceLimit {
        resource,
        limit: u64::MAX,
        observed: u64::MAX,
    })
}

#[cfg(test)]
mod tests {
    use forgedb_core::{InternalKey, Limits, SequenceNumber, ValueType};

    use super::*;

    fn entry(key: &[u8], sequence: u64, value: &[u8]) -> Result<TableEntry> {
        Ok(TableEntry {
            internal_key: InternalKey::new(
                ColumnFamilyId::DEFAULT,
                key,
                SequenceNumber::new(sequence)?,
                ValueType::Value,
                &Limits::default(),
            )?,
            value: value.to_vec(),
        })
    }

    fn collect_events(
        entries: Vec<TableEntry>,
        options: CompactionOutputOptions,
    ) -> Result<Vec<CompactionOutputEvent>> {
        CompactionOutputSplitter::new(entries.into_iter().map(Ok), options)?.collect()
    }

    #[test]
    fn splits_only_between_logical_keys() -> Result<()> {
        let events = collect_events(
            vec![
                entry(b"a", 3, b"1111")?,
                entry(b"a", 2, b"2222")?,
                entry(b"b", 1, b"3333")?,
            ],
            CompactionOutputOptions {
                target_file_size: 80,
                ..CompactionOutputOptions::default()
            },
        )?;
        assert!(matches!(
            events.first(),
            Some(CompactionOutputEvent::BeginFile { output_index: 0 })
        ));
        let finishes = events
            .iter()
            .filter_map(|event| match event {
                CompactionOutputEvent::FinishFile(summary) => Some(summary),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(finishes.len(), 2);
        assert_eq!(finishes[0].entry_count, 2);
        assert_eq!(finishes[0].smallest_user_key, b"a");
        assert_eq!(finishes[0].largest_user_key, b"a");
        assert_eq!(finishes[1].entry_count, 1);
        assert_eq!(finishes[1].smallest_user_key, b"b");
        Ok(())
    }

    #[test]
    fn oversized_key_group_stays_in_one_file() -> Result<()> {
        let entries = vec![
            entry(b"same", 3, &[1; 24])?,
            entry(b"same", 2, &[2; 24])?,
            entry(b"same", 1, &[3; 24])?,
        ];
        let mut splitter = CompactionOutputSplitter::new(
            entries.into_iter().map(Ok),
            CompactionOutputOptions {
                target_file_size: 64,
                max_key_group_bytes: 512,
                ..CompactionOutputOptions::default()
            },
        )?;
        let events = splitter.by_ref().collect::<Result<Vec<_>>>()?;
        let summaries = events
            .iter()
            .filter_map(|event| match event {
                CompactionOutputEvent::FinishFile(summary) => Some(summary),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].entry_count, 3);
        assert!(summaries[0].oversized_key_group);
        assert_eq!(splitter.stats().oversized_key_groups, 1);
        Ok(())
    }

    #[test]
    fn key_group_byte_limit_is_enforced() -> Result<()> {
        let entries = vec![
            entry(b"same", 2, &[1; 32])?,
            entry(b"same", 1, &[2; 32])?,
        ];
        let mut splitter = CompactionOutputSplitter::new(
            entries.into_iter().map(Ok),
            CompactionOutputOptions {
                target_file_size: 64,
                max_key_group_bytes: 80,
                ..CompactionOutputOptions::default()
            },
        )?;
        assert!(matches!(
            splitter.next(),
            Some(Err(Error::ResourceLimit { .. }))
        ));
        assert!(splitter.next().is_none());
        Ok(())
    }

    #[test]
    fn invalid_source_order_is_rejected() -> Result<()> {
        let entries = vec![entry(b"b", 1, b"b")?, entry(b"a", 1, b"a")?];
        let mut splitter = CompactionOutputSplitter::new(
            entries.into_iter().map(Ok),
            CompactionOutputOptions::default(),
        )?;
        assert!(matches!(splitter.next(), Some(Err(Error::Corruption { .. }))));
        Ok(())
    }

    #[test]
    fn empty_input_produces_no_files() -> Result<()> {
        let mut splitter = CompactionOutputSplitter::new(
            std::iter::empty(),
            CompactionOutputOptions::default(),
        )?;
        assert!(splitter.next().is_none());
        assert_eq!(splitter.stats(), CompactionOutputStats::default());
        Ok(())
    }
}
