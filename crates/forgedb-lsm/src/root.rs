//! LSM orchestration ForgeDB: compaction planning, bounded merge и output splitting.

mod merge;
mod output;
#[path = "lib.rs"]
mod planner;

pub use merge::{
    CompactionCursor, CompactionMerge, CompactionMergeOptions, CompactionMergeStats,
    VecCompactionCursor,
};
pub use output::{
    CompactionOutputEvent, CompactionOutputOptions, CompactionOutputSplitter,
    CompactionOutputStats, CompactionOutputSummary,
};
pub use planner::{
    CompactionConfig, CompactionDecision, CompactionPlan, CompactionReason, CompactionScore,
    pick_compaction,
};
