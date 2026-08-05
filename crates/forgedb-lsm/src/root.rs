//! LSM orchestration ForgeDB: compaction planning и bounded merge.

mod merge;
#[path = "lib.rs"]
mod planner;

pub use merge::{
    CompactionCursor, CompactionMerge, CompactionMergeOptions, CompactionMergeStats,
    VecCompactionCursor,
};
pub use planner::{
    CompactionConfig, CompactionDecision, CompactionPlan, CompactionReason, CompactionScore,
    pick_compaction,
};
