//! LSM orchestration ForgeDB: compaction planning и bounded merge.

#[path = "lib.rs"]
mod planner;
mod merge;

pub use merge::{
    CompactionCursor, CompactionMerge, CompactionMergeOptions, CompactionMergeStats,
    VecCompactionCursor,
};
pub use planner::{
    CompactionConfig, CompactionDecision, CompactionPlan, CompactionReason, CompactionScore,
    pick_compaction,
};
