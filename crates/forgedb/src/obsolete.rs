use std::sync::Weak;

use forgedb_sstable::TableReader;

/// Итог консервативной очистки SSTable, выведенных из live MANIFEST topology.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanupReport {
    /// Файлы, физически удалённые при этом вызове.
    pub removed: Vec<String>,
    /// Файлы, удерживаемые работающими читателями.
    pub deferred: Vec<String>,
    /// Файлы, уже отсутствовавшие при попытке безопасной очистки.
    pub absent: Vec<String>,
    /// Ошибки удаления: файлы оставлены в очереди для повторной попытки.
    pub failed: Vec<String>,
}

/// Отдельный SSTable, исключённый из live topology synced compaction edit.
/// Weak<TableReader> отслеживает все версии, разделяющие reader этого файла,
/// включая итераторы, созданные ещё до последующих flush/compaction.
#[derive(Debug)]
pub(crate) struct RetiredSstable {
    pub(crate) file_number: u64,
    pub(crate) reader: Weak<TableReader>,
}
