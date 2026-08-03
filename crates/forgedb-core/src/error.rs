use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// The public ForgeDB error model.
#[derive(Debug)]
pub enum Error {
    /// An operating-system I/O operation failed.
    Io {
        /// Stable operation name.
        operation: &'static str,
        /// Path involved in the operation, when safe and available.
        path: Option<PathBuf>,
        /// Original I/O error.
        source: io::Error,
    },
    /// Persisted or encoded data violated its format contract.
    Corruption {
        /// File containing the problem, when applicable.
        file: Option<PathBuf>,
        /// Byte offset containing the problem, when applicable.
        offset: Option<u64>,
        /// Stable component name.
        component: &'static str,
        /// Human-readable reason without raw values.
        reason: String,
    },
    /// A persisted format version is outside the supported range.
    UnsupportedFormat {
        /// Stable format component name.
        component: &'static str,
        /// Version found in the input.
        found: u32,
        /// Oldest supported version.
        minimum_supported: u32,
        /// Newest supported version.
        maximum_supported: u32,
    },
    /// A caller supplied an invalid argument or configuration value.
    InvalidArgument {
        /// Stable field name.
        field: &'static str,
        /// Human-readable reason.
        reason: String,
    },
    /// A database or resource is currently busy.
    Busy {
        /// Human-readable reason.
        reason: String,
    },
    /// An optimistic transaction detected conflicts.
    Conflict {
        /// Conflicting binary keys. Callers should avoid logging them verbatim.
        keys: Vec<Vec<u8>>,
    },
    /// A configured or structural resource limit was exceeded.
    ResourceLimit {
        /// Stable resource name.
        resource: &'static str,
        /// Configured or structural limit.
        limit: u64,
        /// Observed value.
        observed: u64,
    },
    /// The database is read-only.
    ReadOnly,
    /// The database has been closed.
    Closed,
    /// A background storage component entered a fatal state.
    BackgroundFailure {
        /// Stable component name.
        component: &'static str,
        /// Human-readable reason.
        reason: String,
    },
    /// An internal invariant was violated.
    InternalInvariant {
        /// Stable invariant identifier.
        invariant: &'static str,
        /// Human-readable reason.
        reason: String,
    },
}

impl Error {
    /// Creates an I/O error with operation and path context.
    #[must_use]
    pub fn io(operation: &'static str, path: impl Into<Option<PathBuf>>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    /// Creates a corruption error without file context.
    #[must_use]
    pub fn corruption(component: &'static str, reason: impl Into<String>) -> Self {
        Self::Corruption {
            file: None,
            offset: None,
            component,
            reason: reason.into(),
        }
    }

    /// Creates an invalid-argument error.
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
                    write!(formatter, "I/O error during {operation} at {}: {source}", path.display())
                } else {
                    write!(formatter, "I/O error during {operation}: {source}")
                }
            }
            Self::Corruption {
                file,
                offset,
                component,
                reason,
            } => {
                write!(formatter, "corruption in {component}")?;
                if let Some(file) = file {
                    write!(formatter, " at {}", file.display())?;
                }
                if let Some(offset) = offset {
                    write!(formatter, " offset {offset}")?;
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
                "unsupported {component} format version {found}; supported range is {minimum_supported}..={maximum_supported}"
            ),
            Self::InvalidArgument { field, reason } => {
                write!(formatter, "invalid argument {field}: {reason}")
            }
            Self::Busy { reason } => write!(formatter, "resource is busy: {reason}"),
            Self::Conflict { keys } => {
                write!(formatter, "transaction conflict on {} key(s)", keys.len())
            }
            Self::ResourceLimit {
                resource,
                limit,
                observed,
            } => write!(
                formatter,
                "resource limit exceeded for {resource}: observed {observed}, limit {limit}"
            ),
            Self::ReadOnly => formatter.write_str("database is read-only"),
            Self::Closed => formatter.write_str("database is closed"),
            Self::BackgroundFailure { component, reason } => {
                write!(formatter, "background failure in {component}: {reason}")
            }
            Self::InternalInvariant { invariant, reason } => {
                write!(formatter, "internal invariant {invariant} failed: {reason}")
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

/// ForgeDB result alias.
pub type Result<T> = std::result::Result<T, Error>;
