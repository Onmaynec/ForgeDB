use std::collections::BTreeSet;

use forgedb_core::{Error, Result};
use forgedb_sstable::TableMeta;

use crate::planner::{
    CompactionConfig, CompactionDecision, CompactionPlan, CompactionReason, CompactionScore,
    extend_range, ranges_overlap, validate_layout,
};

/// Выбирает одну deterministic compaction, которая пересекает manual half-open range.
///
/// Manual selection не требует background score >= 1. Для L0 строится полное
/// overlap closure через L1. Для L1+ выбирается первый пересекающий source file
/// и все его overlaps в следующем level.
pub fn pick_manual_compaction(
    levels: &[Vec<TableMeta>],
    config: CompactionConfig,
    start: Option<&[u8]>,
    end: Option<&[u8]>,
) -> Result<CompactionDecision> {
    config.validate(levels.len())?;
    validate_layout(levels)?;
    if start.zip(end).is_some_and(|(start, end)| start > end) {
        return Err(Error::invalid_argument(
            "manual_compaction_range",
            "start больше end",
        ));
    }
    if start.zip(end).is_some_and(|(start, end)| start == end) {
        return Ok(CompactionDecision::None);
    }

    for source_level in 0..levels.len().saturating_sub(1) {
        let source = levels.get(source_level).ok_or(Error::InternalInvariant {
            invariant: "I-MANUAL-COMPACTION-SOURCE-LEVEL",
            reason: "source level отсутствует после layout validation".to_owned(),
        })?;
        let Some(seed) = source
            .iter()
            .find(|file| file_overlaps_request(file, start, end))
        else {
            continue;
        };
        let score = CompactionScore {
            level: source_level,
            numerator: 1,
            denominator: 1,
            reason: CompactionReason::ManualRange,
        };
        let plan = if source_level == 0 {
            manual_l0_plan(levels, seed.file_number, score)?
        } else {
            manual_level_plan(levels, seed.file_number, score)?
        };
        return bounded_decision(plan, config.max_compaction_bytes);
    }
    Ok(CompactionDecision::None)
}

fn manual_l0_plan(
    levels: &[Vec<TableMeta>],
    seed_number: u64,
    score: CompactionScore,
) -> Result<CompactionPlan> {
    let level_zero = levels.first().ok_or(Error::InternalInvariant {
        invariant: "I-MANUAL-COMPACTION-L0",
        reason: "layout не содержит L0".to_owned(),
    })?;
    let level_one = levels.get(1).ok_or(Error::InternalInvariant {
        invariant: "I-MANUAL-COMPACTION-L1",
        reason: "layout не содержит L1".to_owned(),
    })?;
    let seed = level_zero
        .iter()
        .find(|file| file.file_number == seed_number)
        .ok_or(Error::InternalInvariant {
            invariant: "I-MANUAL-COMPACTION-L0-SEED",
            reason: "выбранный L0 seed отсутствует".to_owned(),
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
    build_manual_plan(BuildManualPlanRequest {
        source_level: 0,
        output_level: 1,
        source: level_zero,
        target: level_one,
        selected_source: &selected_l0,
        selected_target: &selected_l1,
        smallest_user_key: smallest,
        largest_user_key: largest,
        score,
    })
}

fn manual_level_plan(
    levels: &[Vec<TableMeta>],
    seed_number: u64,
    score: CompactionScore,
) -> Result<CompactionPlan> {
    let source = levels.get(score.level).ok_or(Error::InternalInvariant {
        invariant: "I-MANUAL-COMPACTION-SOURCE",
        reason: "selected source level отсутствует".to_owned(),
    })?;
    let output_level = score.level.checked_add(1).ok_or(Error::ResourceLimit {
        resource: "manual_compaction_output_level",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    let target = levels.get(output_level).ok_or(Error::InternalInvariant {
        invariant: "I-MANUAL-COMPACTION-TARGET",
        reason: "selected output level отсутствует".to_owned(),
    })?;
    let seed = source
        .iter()
        .find(|file| file.file_number == seed_number)
        .ok_or(Error::InternalInvariant {
            invariant: "I-MANUAL-COMPACTION-SEED",
            reason: "выбранный source seed отсутствует".to_owned(),
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
    build_manual_plan(BuildManualPlanRequest {
        source_level: score.level,
        output_level,
        source,
        target,
        selected_source: &selected_source,
        selected_target: &selected_target,
        smallest_user_key: smallest,
        largest_user_key: largest,
        score,
    })
}

struct BuildManualPlanRequest<'a> {
    source_level: usize,
    output_level: usize,
    source: &'a [TableMeta],
    target: &'a [TableMeta],
    selected_source: &'a BTreeSet<u64>,
    selected_target: &'a BTreeSet<u64>,
    smallest_user_key: Vec<u8>,
    largest_user_key: Vec<u8>,
    score: CompactionScore,
}

fn build_manual_plan(request: BuildManualPlanRequest<'_>) -> Result<CompactionPlan> {
    let BuildManualPlanRequest {
        source_level,
        output_level,
        source,
        target,
        selected_source,
        selected_target,
        smallest_user_key,
        largest_user_key,
        score,
    } = request;
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
    let total_input_bytes = source
        .iter()
        .filter(|file| selected_source.contains(&file.file_number))
        .chain(
            target
                .iter()
                .filter(|file| selected_target.contains(&file.file_number)),
        )
        .try_fold(0_u64, |total, file| {
            total
                .checked_add(file.file_size)
                .ok_or(Error::ResourceLimit {
                    resource: "manual_compaction_input_bytes",
                    limit: u64::MAX,
                    observed: u64::MAX,
                })
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

fn bounded_decision(plan: CompactionPlan, max_compaction_bytes: u64) -> Result<CompactionDecision> {
    if plan.total_input_bytes > max_compaction_bytes {
        return Ok(CompactionDecision::Blocked {
            plan,
            max_compaction_bytes,
        });
    }
    Ok(CompactionDecision::Plan(plan))
}

fn file_overlaps_request(file: &TableMeta, start: Option<&[u8]>, end: Option<&[u8]>) -> bool {
    if start.is_some_and(|start| file.largest_user_key.as_slice() < start) {
        return false;
    }
    if end.is_some_and(|end| file.smallest_user_key.as_slice() >= end) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use forgedb_core::SequenceNumber;

    use super::*;

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

    fn config() -> CompactionConfig {
        CompactionConfig {
            l0_compaction_trigger: 10,
            level1_target_size: 1_000_000,
            level_size_multiplier: 10,
            max_compaction_bytes: 10_000,
        }
    }

    #[test]
    fn range_selects_intersecting_l0_seed_below_background_trigger() -> Result<()> {
        let levels = vec![
            vec![table(10, 100, b"a", b"b"), table(11, 100, b"m", b"n")],
            Vec::new(),
            Vec::new(),
        ];
        let CompactionDecision::Plan(plan) =
            pick_manual_compaction(&levels, config(), Some(b"m"), Some(b"z"))?
        else {
            return Err(Error::InternalInvariant {
                invariant: "I-TEST-MANUAL-L0-PLAN",
                reason: "ожидался manual L0 plan".to_owned(),
            });
        };
        assert_eq!(plan.source_files, vec![11]);
        assert_eq!(plan.score.reason, CompactionReason::ManualRange);
        Ok(())
    }

    #[test]
    fn non_intersecting_high_pressure_level_does_not_starve_requested_range() -> Result<()> {
        let levels = vec![
            vec![
                table(10, 100, b"a", b"b"),
                table(11, 100, b"c", b"d"),
                table(12, 100, b"e", b"f"),
                table(13, 100, b"g", b"h"),
            ],
            vec![table(20, 100, b"m", b"n")],
            Vec::new(),
        ];
        let CompactionDecision::Plan(plan) =
            pick_manual_compaction(&levels, config(), Some(b"m"), Some(b"z"))?
        else {
            return Err(Error::InternalInvariant {
                invariant: "I-TEST-MANUAL-RANGE-PLAN",
                reason: "ожидался manual range plan".to_owned(),
            });
        };
        assert_eq!(plan.source_level, 1);
        assert_eq!(plan.source_files, vec![20]);
        Ok(())
    }

    #[test]
    fn empty_or_missing_range_returns_none() -> Result<()> {
        let levels = vec![vec![table(10, 100, b"a", b"b")], Vec::new()];
        assert_eq!(
            pick_manual_compaction(&levels, config(), Some(b"x"), Some(b"z"))?,
            CompactionDecision::None
        );
        assert_eq!(
            pick_manual_compaction(&levels, config(), Some(b"a"), Some(b"a"))?,
            CompactionDecision::None
        );
        Ok(())
    }
}
