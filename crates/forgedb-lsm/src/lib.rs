//! Deterministic LSM compaction planning for ForgeDB.
//!
//! Этот crate пока не изменяет persisted state. Он вычисляет compaction score,
//! выбирает входные SSTable и явно сообщает, когда безопасное overlap closure
//! превышает configured byte limit.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use forgedb_core::{Error, Result};
use forgedb_sstable::TableMeta;

/// Политика выбора compaction inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompactionConfig {
    pub l0_compaction_trigger: usize,
    pub level1_target_size: u64,
    pub level_size_multiplier: u64,
    pub max_compaction_bytes: u64,
}

impl CompactionConfig {
    /// Проверяет конфигурацию и все вычисляемые level targets.
    pub fn validate(self, level_count: usize) -> Result<()> {
        if level_count < 2 {
            return Err(Error::invalid_argument(
                "level_count",
                "compaction требует минимум L0 и L1",
            ));
        }
        if self.l0_compaction_trigger == 0 {
            return Err(Error::invalid_argument(
                "l0_compaction_trigger",
                "значение должно быть больше нуля",
            ));
        }
        if self.level1_target_size == 0 {
            return Err(Error::invalid_argument(
                "level1_target_size",
                "значение должно быть больше нуля",
            ));
        }
        if self.level_size_multiplier < 2 {
            return Err(Error::invalid_argument(
                "level_size_multiplier",
                "значение должно быть не меньше 2",
            ));
        }
        if self.max_compaction_bytes == 0 {
            return Err(Error::invalid_argument(
                "max_compaction_bytes",
                "значение должно быть больше нуля",
            ));
        }
        for level in 1..level_count {
            let _ = self.level_target(level)?;
        }
        Ok(())
    }

    fn level_target(self, level: usize) -> Result<u64> {
        if level == 0 {
            return Err(Error::invalid_argument(
                "level",
                "L0 использует file-count score, а не byte target",
            ));
        }
        let mut target = self.level1_target_size;
        for _ in 1..level {
            target = target
                .checked_mul(self.level_size_multiplier)
                .ok_or_else(|| {
                    Error::invalid_argument(
                        "level_size_multiplier",
                        "рассчитанный level target переполняет u64",
                    )
                })?;
        }
        Ok(target)
    }
}

/// Причина, по которой level стал eligible для compaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactionReason {
    L0FileCount,
    LevelBytes,
}

/// Точный rational score без floating-point rounding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompactionScore {
    pub level: usize,
    pub numerator: u64,
    pub denominator: u64,
    pub reason: CompactionReason,
}

impl CompactionScore {
    #[must_use]
    pub fn ratio_cmp(self, other: Self) -> Ordering {
        let left = u128::from(self.numerator) * u128::from(other.denominator);
        let right = u128::from(other.numerator) * u128::from(self.denominator);
        left.cmp(&right)
    }
}

/// Полный deterministic input set одной compaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionPlan {
    pub source_level: usize,
    pub output_level: usize,
    pub source_files: Vec<u64>,
    pub output_level_files: Vec<u64>,
    pub smallest_user_key: Vec<u8>,
    pub largest_user_key: Vec<u8>,
    pub total_input_bytes: u64,
    pub score: CompactionScore,
}

/// Результат picker без скрытого fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompactionDecision {
    None,
    Plan(CompactionPlan),
    Blocked {
        plan: CompactionPlan,
        max_compaction_bytes: u64,
    },
}

/// Выбирает highest eligible level и строит безопасный input set.
///
/// `levels[0]` должен быть упорядочен по возрастающему file number.
/// `levels[1..]` должны быть упорядочены по key range и не перекрываться.
pub fn pick_compaction(
    levels: &[Vec<TableMeta>],
    config: CompactionConfig,
) -> Result<CompactionDecision> {
    config.validate(levels.len())?;
    validate_layout(levels)?;

    let Some(score) = highest_score(levels, config)? else {
        return Ok(CompactionDecision::None);
    };
    let plan = if score.level == 0 {
        pick_l0_to_l1(levels, score)?
    } else {
        pick_level_to_next(levels, score)?
    };
    if plan.total_input_bytes > config.max_compaction_bytes {
        return Ok(CompactionDecision::Blocked {
            plan,
            max_compaction_bytes: config.max_compaction_bytes,
        });
    }
    Ok(CompactionDecision::Plan(plan))
}

fn highest_score(
    levels: &[Vec<TableMeta>],
    config: CompactionConfig,
) -> Result<Option<CompactionScore>> {
    let mut best: Option<CompactionScore> = None;
    for (level, files) in levels.iter().enumerate() {
        if level + 1 >= levels.len() {
            break;
        }
        let candidate = if level == 0 {
            let file_count = u64::try_from(files.len()).map_err(|_| Error::ResourceLimit {
                resource: "l0_file_count",
                limit: u64::MAX,
                observed: u64::MAX,
            })?;
            let trigger = u64::try_from(config.l0_compaction_trigger).map_err(|_| {
                Error::ResourceLimit {
                    resource: "l0_compaction_trigger",
                    limit: u64::MAX,
                    observed: u64::MAX,
                }
            })?;
            (file_count >= trigger).then_some(CompactionScore {
                level,
                numerator: file_count,
                denominator: trigger,
                reason: CompactionReason::L0FileCount,
            })
        } else {
            let bytes = total_bytes(files)?;
            let target = config.level_target(level)?;
            (bytes >= target).then_some(CompactionScore {
                level,
                numerator: bytes,
                denominator: target,
                reason: CompactionReason::LevelBytes,
            })
        };
        let Some(candidate) = candidate else {
            continue;
        };
        let replace = match best {
            None => true,
            Some(current) => match candidate.ratio_cmp(current) {
                Ordering::Greater => true,
                Ordering::Equal => candidate.level < current.level,
                Ordering::Less => false,
            },
        };
        if replace {
            best = Some(candidate);
        }
    }
    Ok(best)
}

fn pick_l0_to_l1(levels: &[Vec<TableMeta>], score: CompactionScore) -> Result<CompactionPlan> {
    let level_zero = levels.first().ok_or(Error::InternalInvariant {
        invariant: "I-COMPACTION-L0-PRESENT",
        reason: "layout не содержит L0".to_owned(),
    })?;
    let level_one = levels.get(1).ok_or(Error::InternalInvariant {
        invariant: "I-COMPACTION-L1-PRESENT",
        reason: "layout не содержит L1".to_owned(),
    })?;
    let seed = level_zero.first().ok_or(Error::InternalInvariant {
        invariant: "I-COMPACTION-L0-SEED",
        reason: "eligible L0 не содержит файлов".to_owned(),
    })?;

    let mut smallest = seed.smallest_user_key.clone();
    let mut largest = seed.largest_user_key.clone();
    let mut selected_l0 = BTreeSet::new();
    let mut selected_l1 = BTreeSet::new();

    loop {
        let before_l0 = selected_l0.len();
        let before_l1 = selected_l1.len();
        for file in level_zero {
            if ranges_overlap(
                &smallest,
                &largest,
                &file.smallest_user_key,
                &file.largest_user_key,
            ) {
                selected_l0.insert(file.file_number);
                extend_range(&mut smallest, &mut largest, file);
            }
        }
        for file in level_one {
            if ranges_overlap(
                &smallest,
                &largest,
                &file.smallest_user_key,
                &file.largest_user_key,
            ) {
                selected_l1.insert(file.file_number);
                extend_range(&mut smallest, &mut largest, file);
            }
        }
        if before_l0 == selected_l0.len() && before_l1 == selected_l1.len() {
            break;
        }
    }

    build_plan(
        0,
        1,
        level_zero,
        level_one,
        &selected_l0,
        &selected_l1,
        smallest,
        largest,
        score,
    )
}

fn pick_level_to_next(
    levels: &[Vec<TableMeta>],
    score: CompactionScore,
) -> Result<CompactionPlan> {
    let source = levels.get(score.level).ok_or(Error::InternalInvariant {
        invariant: "I-COMPACTION-SOURCE-LEVEL",
        reason: "selected source level отсутствует".to_owned(),
    })?;
    let output_level = score.level.checked_add(1).ok_or(Error::ResourceLimit {
        resource: "compaction_output_level",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    let target = levels.get(output_level).ok_or(Error::InternalInvariant {
        invariant: "I-COMPACTION-OUTPUT-LEVEL",
        reason: "selected output level отсутствует".to_owned(),
    })?;
    let seed = source.first().ok_or(Error::InternalInvariant {
        invariant: "I-COMPACTION-SOURCE-SEED",
        reason: "eligible source level не содержит файлов".to_owned(),
    })?;

    let selected_source = BTreeSet::from([seed.file_number]);
    let selected_target = target
        .iter()
        .filter(|file| {
            ranges_overlap(
                &seed.smallest_user_key,
                &seed.largest_user_key,
                &file.smallest_user_key,
                &file.largest_user_key,
            )
        })
        .map(|file| file.file_number)
        .collect::<BTreeSet<_>>();
    let mut smallest = seed.smallest_user_key.clone();
    let mut largest = seed.largest_user_key.clone();
    for file in target {
        if selected_target.contains(&file.file_number) {
            extend_range(&mut smallest, &mut largest, file);
        }
    }

    build_plan(
        score.level,
        output_level,
        source,
        target,
        &selected_source,
        &selected_target,
        smallest,
        largest,
        score,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_plan(
    source_level: usize,
    output_level: usize,
    source: &[TableMeta],
    target: &[TableMeta],
    selected_source: &BTreeSet<u64>,
    selected_target: &BTreeSet<u64>,
    smallest_user_key: Vec<u8>,
    largest_user_key: Vec<u8>,
    score: CompactionScore,
) -> Result<CompactionPlan> {
    let source_files = source
        .iter()
        .filter(|file| selected_source.contains(&file.file_number))
        .map(|file| file.file_number)
        .collect::<Vec<_>>();
    let output_level_files = target
        .iter()
        .filter(|file| selected_target.contains(&file.file_number))
        .map(|file| file.file_number)
        .collect::<Vec<_>>();
    let source_bytes = selected_bytes(source, selected_source)?;
    let target_bytes = selected_bytes(target, selected_target)?;
    let total_input_bytes = source_bytes
        .checked_add(target_bytes)
        .ok_or(Error::ResourceLimit {
            resource: "compaction_input_bytes",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    Ok(CompactionPlan {
        source_level,
        output_level,
        source_files,
        output_level_files,
        smallest_user_key,
        largest_user_key,
        total_input_bytes,
        score,
    })
}

fn validate_layout(levels: &[Vec<TableMeta>]) -> Result<()> {
    let mut numbers = BTreeSet::new();
    for (level, files) in levels.iter().enumerate() {
        for file in files {
            if file.file_number == 0 || file.file_size == 0 {
                return Err(Error::corruption(
                    "compaction layout",
                    "file number и file size должны быть ненулевыми",
                ));
            }
            if file.smallest_user_key.is_empty()
                || file.smallest_user_key > file.largest_user_key
            {
                return Err(Error::corruption(
                    "compaction layout",
                    "некорректный user-key range SSTable",
                ));
            }
            if !numbers.insert(file.file_number) {
                return Err(Error::corruption(
                    "compaction layout",
                    "duplicate SSTable file number",
                ));
            }
        }
        if level == 0 {
            if files
                .windows(2)
                .any(|pair| pair[0].file_number >= pair[1].file_number)
            {
                return Err(Error::corruption(
                    "compaction layout L0",
                    "L0 file numbers не строго возрастают",
                ));
            }
        } else if files.windows(2).any(|pair| {
            pair[0].smallest_user_key >= pair[1].smallest_user_key
                || pair[0].largest_user_key >= pair[1].smallest_user_key
        }) {
            return Err(Error::corruption(
                "compaction layout L1+",
                "level files не отсортированы или перекрываются",
            ));
        }
    }
    Ok(())
}

fn total_bytes(files: &[TableMeta]) -> Result<u64> {
    files.iter().try_fold(0_u64, |total, file| {
        total.checked_add(file.file_size).ok_or(Error::ResourceLimit {
            resource: "level_bytes",
            limit: u64::MAX,
            observed: u64::MAX,
        })
    })
}

fn selected_bytes(files: &[TableMeta], selected: &BTreeSet<u64>) -> Result<u64> {
    files
        .iter()
        .filter(|file| selected.contains(&file.file_number))
        .try_fold(0_u64, |total, file| {
            total.checked_add(file.file_size).ok_or(Error::ResourceLimit {
                resource: "compaction_input_bytes",
                limit: u64::MAX,
                observed: u64::MAX,
            })
        })
}

fn ranges_overlap(left_start: &[u8], left_end: &[u8], right_start: &[u8], right_end: &[u8]) -> bool {
    left_start <= right_end && right_start <= left_end
}

fn extend_range(smallest: &mut Vec<u8>, largest: &mut Vec<u8>, file: &TableMeta) {
    if file.smallest_user_key < *smallest {
        *smallest = file.smallest_user_key.clone();
    }
    if file.largest_user_key > *largest {
        *largest = file.largest_user_key.clone();
    }
}

#[cfg(test)]
mod tests {
    use forgedb_core::SequenceNumber;

    use super::*;

    fn config() -> CompactionConfig {
        CompactionConfig {
            l0_compaction_trigger: 4,
            level1_target_size: 1_000,
            level_size_multiplier: 10,
            max_compaction_bytes: 10_000,
        }
    }

    fn table(number: u64, size: u64, smallest: &[u8], largest: &[u8]) -> TableMeta {
        TableMeta {
            file_number: number,
            file_size: size,
            smallest_user_key: smallest.to_vec(),
            largest_user_key: largest.to_vec(),
            smallest_sequence: SequenceNumber::FIRST,
            largest_sequence: SequenceNumber::FIRST,
            entry_count: 1,
        }
    }

    #[test]
    fn no_level_above_target_returns_none() -> Result<()> {
        let levels = vec![
            vec![table(10, 100, b"a", b"b")],
            vec![table(20, 900, b"c", b"d")],
            Vec::new(),
        ];
        assert_eq!(
            pick_compaction(&levels, config())?,
            CompactionDecision::None
        );
        Ok(())
    }

    #[test]
    fn higher_score_wins_and_ties_prefer_lower_level() -> Result<()> {
        let levels = vec![
            vec![
                table(10, 100, b"a", b"b"),
                table(11, 100, b"c", b"d"),
                table(12, 100, b"e", b"f"),
                table(13, 100, b"g", b"h"),
            ],
            vec![table(20, 1_000, b"i", b"j")],
            Vec::new(),
        ];
        let CompactionDecision::Plan(plan) = pick_compaction(&levels, config())? else {
            return Err(Error::InternalInvariant {
                invariant: "I-TEST-COMPACTION-PLAN",
                reason: "ожидался compaction plan".to_owned(),
            });
        };
        assert_eq!(plan.source_level, 0);
        assert_eq!(plan.score.numerator, 4);
        assert_eq!(plan.score.denominator, 4);
        Ok(())
    }

    #[test]
    fn l0_overlap_closure_expands_through_l1() -> Result<()> {
        let levels = vec![
            vec![
                table(10, 100, b"a", b"c"),
                table(11, 100, b"f", b"h"),
                table(12, 100, b"x", b"z"),
                table(13, 100, b"m", b"n"),
            ],
            vec![table(20, 200, b"b", b"g")],
            Vec::new(),
        ];
        let CompactionDecision::Plan(plan) = pick_compaction(&levels, config())? else {
            return Err(Error::InternalInvariant {
                invariant: "I-TEST-L0-CLOSURE",
                reason: "ожидался L0 compaction plan".to_owned(),
            });
        };
        assert_eq!(plan.source_files, vec![10, 11]);
        assert_eq!(plan.output_level_files, vec![20]);
        assert_eq!(plan.smallest_user_key, b"a");
        assert_eq!(plan.largest_user_key, b"h");
        Ok(())
    }

    #[test]
    fn level_compaction_selects_overlapping_next_level_files() -> Result<()> {
        let levels = vec![
            Vec::new(),
            vec![table(20, 1_200, b"d", b"m")],
            vec![
                table(30, 300, b"a", b"c"),
                table(31, 300, b"g", b"k"),
                table(32, 300, b"n", b"z"),
            ],
        ];
        let CompactionDecision::Plan(plan) = pick_compaction(&levels, config())? else {
            return Err(Error::InternalInvariant {
                invariant: "I-TEST-LEVEL-PLAN",
                reason: "ожидался level compaction plan".to_owned(),
            });
        };
        assert_eq!(plan.source_level, 1);
        assert_eq!(plan.output_level, 2);
        assert_eq!(plan.source_files, vec![20]);
        assert_eq!(plan.output_level_files, vec![31]);
        Ok(())
    }

    #[test]
    fn oversized_overlap_closure_is_explicitly_blocked() -> Result<()> {
        let levels = vec![
            vec![
                table(10, 400, b"a", b"c"),
                table(11, 400, b"b", b"d"),
                table(12, 400, b"x", b"y"),
                table(13, 400, b"z", b"zz"),
            ],
            vec![table(20, 400, b"c", b"e")],
            Vec::new(),
        ];
        let limited = CompactionConfig {
            max_compaction_bytes: 1_000,
            ..config()
        };
        let CompactionDecision::Blocked {
            plan,
            max_compaction_bytes,
        } = pick_compaction(&levels, limited)?
        else {
            return Err(Error::InternalInvariant {
                invariant: "I-TEST-COMPACTION-BLOCKED",
                reason: "ожидался explicit blocker".to_owned(),
            });
        };
        assert_eq!(max_compaction_bytes, 1_000);
        assert_eq!(plan.total_input_bytes, 1_200);
        Ok(())
    }

    #[test]
    fn overflowing_level_target_is_rejected() {
        let invalid = CompactionConfig {
            level1_target_size: u64::MAX / 2,
            level_size_multiplier: 10,
            ..config()
        };
        assert!(invalid.validate(4).is_err());
    }
}
