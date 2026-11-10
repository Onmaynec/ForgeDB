use std::sync::Weak;

use crate::version_set::VersionSet;

/// Итог консервативной очистки SSTable, выведенных из live MANIFEST topology.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanupReport {
    /// Файлы, физически удалённые при этом вызове.
    pub removed: Vec<String>,
    /// Файлы, удерживаемые старым VersionSet или работающими читателями.
    pub deferred: Vec<String>,
    /// Файлы, уже отсутствовавшие при попытке безопасной очистки.
    pub absent: Vec<String>,
    /// Ошибки удаления: файлы оставлены в очереди для повторной попытки.
    pub failed: Vec<String>,
}

/// Только файлы, которые исключены из live topology синхронизированным
/// compaction VersionEdit в этом open handle. После restart выводов
/// об orphan-файлах этот список не делает.
#[derive(Debug)]
pub(crate) struct RetiredSstables {
    pub(crate) old_version: Weak<VersionSet>,
    pub(crate) file_numbers: Vec<u64>,
}
