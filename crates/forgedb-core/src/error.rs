use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Публичная модель ошибок ForgeDB.
#[derive(Debug)]
pub enum Error {
    /// Системная операция ввода-вывода завершилась ошибкой.
    Io {
        /// Стабильное имя операции.
        operation: &'static str,
        /// Путь операции, когда его безопасно и возможно предоставить.
        path: Option<PathBuf>,
        /// Исходная ошибка ввода-вывода.
        source: io::Error,
    },
    /// Persisted или encoded данные нарушили контракт формата.
    Corruption {
        /// Файл с проблемой, когда применимо.
        file: Option<PathBuf>,
        /// Byte offset проблемы, когда применимо.
        offset: Option<u64>,
        /// Стабильное имя компонента.
        component: &'static str,
        /// Читаемая причина без raw values.
        reason: String,
    },
    /// Версия persisted-формата находится вне поддерживаемого диапазона.
    UnsupportedFormat {
        /// Стабильное имя компонента формата.
        component: &'static str,
        /// Версия, найденная во входных данных.
        found: u32,
        /// Самая старая поддерживаемая версия.
        minimum_supported: u32,
        /// Самая новая поддерживаемая версия.
        maximum_supported: u32,
    },
    /// Caller передал неверный аргумент или значение конфигурации.
    InvalidArgument {
        /// Стабильное имя поля.
        field: &'static str,
        /// Читаемая причина.
        reason: String,
    },
    /// База или ресурс сейчас заняты.
    Busy {
        /// Читаемая причина.
        reason: String,
    },
    /// Optimistic transaction обнаружила конфликты.
    Conflict {
        /// Конфликтующие бинарные ключи; caller не должен безусловно писать их в log.
        keys: Vec<Vec<u8>>,
    },
    /// Настроенный или структурный resource limit превышен.
    ResourceLimit {
        /// Стабильное имя ресурса.
        resource: &'static str,
        /// Настроенная или структурная граница.
        limit: u64,
        /// Наблюдаемое значение.
        observed: u64,
    },
    /// База открыта только для чтения.
    ReadOnly,
    /// База закрыта.
    Closed,
    /// Фоновый storage-компонент перешёл в fatal state.
    BackgroundFailure {
        /// Стабильное имя компонента.
        component: &'static str,
        /// Читаемая причина.
        reason: String,
    },
    /// Внутренний инвариант нарушен.
    InternalInvariant {
        /// Стабильный идентификатор инварианта.
        invariant: &'static str,
        /// Читаемая причина.
        reason: String,
    },
}

impl Error {
    /// Создаёт I/O error с контекстом операции и пути.
    #[must_use]
    pub fn io(
        operation: &'static str,
        path: impl Into<Option<PathBuf>>,
        source: io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    /// Создаёт corruption error без контекста файла.
    #[must_use]
    pub fn corruption(component: &'static str, reason: impl Into<String>) -> Self {
        Self::Corruption {
            file: None,
            offset: None,
            component,
            reason: reason.into(),
        }
    }

    /// Создаёт invalid-argument error.
    #[must_use]
    pub fn invalid_argument(field: &'static str, reason: impl Into<String>) -> Self {
        Self::InvalidArgument {
            field,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => {
                if let Some(path) = path {
                    write!(
                        formatter,
                        "ошибка I/O во время {operation} по пути {}: {source}",
                        path.display()
                    )
                } else {
                    write!(formatter, "ошибка I/O во время {operation}: {source}")
                }
            }
            Self::Corruption {
                file,
                offset,
                component,
                reason,
            } => {
                write!(formatter, "повреждение данных в {component}")?;
                if let Some(file) = file {
                    write!(formatter, " в файле {}", file.display())?;
                }
                if let Some(offset) = offset {
                    write!(formatter, " по смещению {offset}")?;
                }
                write!(formatter, ": {reason}")
            }
            Self::UnsupportedFormat {
                component,
                found,
                minimum_supported,
                maximum_supported,
            } => write!(
                formatter,
                "неподдерживаемая версия формата {component}: {found}; поддерживается диапазон {minimum_supported}..={maximum_supported}"
            ),
            Self::InvalidArgument { field, reason } => {
                write!(formatter, "неверный аргумент {field}: {reason}")
            }
            Self::Busy { reason } => write!(formatter, "ресурс занят: {reason}"),
            Self::Conflict { keys } => {
                write!(formatter, "конфликт transaction для {} ключей", keys.len())
            }
            Self::ResourceLimit {
                resource,
                limit,
                observed,
            } => write!(
                formatter,
                "превышен resource limit для {resource}: наблюдается {observed}, предел {limit}"
            ),
            Self::ReadOnly => formatter.write_str("база открыта только для чтения"),
            Self::Closed => formatter.write_str("база закрыта"),
            Self::BackgroundFailure { component, reason } => {
                write!(formatter, "фатальная ошибка компонента {component}: {reason}")
            }
            Self::InternalInvariant { invariant, reason } => {
                write!(formatter, "нарушен внутренний инвариант {invariant}: {reason}")
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Alias результата ForgeDB.
pub type Result<T> = std::result::Result<T, Error>;
