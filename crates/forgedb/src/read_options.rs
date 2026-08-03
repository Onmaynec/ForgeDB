use crate::Snapshot;

/// Настройки point read и range scan.
#[derive(Clone, Debug)]
pub struct ReadOptions {
    /// Явный MVCC snapshot. `None` означает latest visible sequence на момент начала операции.
    pub snapshot: Option<Snapshot>,
    /// Проверять checksums persisted blocks.
    pub verify_checksums: bool,
    /// Разрешить заполнение cache. Cache появится в следующем milestone.
    pub fill_cache: bool,
    /// Максимальное число user entries для range scan.
    pub limit: Option<usize>,
}

impl ReadOptions {
    /// Создаёт options для конкретного snapshot.
    #[must_use]
    pub fn from_snapshot(snapshot: &Snapshot) -> Self {
        Self {
            snapshot: Some(snapshot.clone()),
            ..Self::default()
        }
    }

    /// Задаёт bounded limit range scan.
    #[must_use]
    pub const fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            snapshot: None,
            verify_checksums: true,
            fill_cache: true,
            limit: None,
        }
    }
}
