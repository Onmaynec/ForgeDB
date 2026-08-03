/// Режим подтверждения записи.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Durability {
    /// Возврат только после успешного WAL file sync и применения batch к memtable.
    Sync,
    /// Возврат после append в WAL и применения к memtable без обязательного file sync.
    NoSync,
    /// Применение только к memtable. Данные могут исчезнуть при process crash.
    DisableWal,
}

/// Параметры одной операции записи.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteOptions {
    durability: Durability,
}

impl WriteOptions {
    /// Создаёт параметры с указанным режимом durability.
    #[must_use]
    pub const fn new(durability: Durability) -> Self {
        Self { durability }
    }

    /// Требует синхронного подтверждения WAL.
    #[must_use]
    pub const fn sync() -> Self {
        Self::new(Durability::Sync)
    }

    /// Не требует физической синхронизации WAL перед возвратом.
    #[must_use]
    pub const fn no_sync() -> Self {
        Self::new(Durability::NoSync)
    }

    /// Полностью отключает WAL для этой записи.
    #[must_use]
    pub const fn disable_wal() -> Self {
        Self::new(Durability::DisableWal)
    }

    /// Возвращает выбранный режим.
    #[must_use]
    pub const fn durability(self) -> Durability {
        self.durability
    }
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self::sync()
    }
}
