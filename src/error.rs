//! Errors and the exit-code contract.
//!
//! Exit codes are a contract with callers (and with desire-path, which records
//! what `sd` refuses). A code is never reused or renumbered. 10-18 were the v0
//! shell's per-verb "not yet implemented" codes; they are retired, not free.

use std::fmt;

/// What went wrong, which decides the process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Anything without a more specific code: store I/O, a corrupt store.
    Failed,
    /// The caller asked for something malformed (a bad flag value, `--at` on a
    /// write). clap's own usage errors use the same code.
    Usage,
    /// A seed id (or dependency, or comment) that does not exist.
    NotFound,
    /// A compare-and-set lost: the seed changed since it was read, or a claim
    /// found it already claimed. Nothing was written; re-read and retry.
    Conflict,
    /// The write was well-formed but refused: SHACL shapes, a dependency cycle,
    /// closing a seed with open blockers without `--force`.
    Refused,
    /// The configuration is contradictory or unreadable.
    Config,
    /// The configured quipu server could not be reached. seeds never falls
    /// back to a local store when this happens.
    Unreachable,
    /// A write whose outcome is unknown: the server's response was lost and
    /// a read-back could not confirm the write. It may yet land. Check before
    /// doing anything else; a blind retry can duplicate it.
    Indeterminate,
    /// A configured capability exists in the design but is not built yet.
    NotBuilt,
    /// A br verb whose capability lives in another tool of the stack (quipu,
    /// shuttle, caboodle). The message names where; desire-path records it.
    Elsewhere,
}

impl ErrorKind {
    /// Every kind, for the published error schema.
    pub const ALL: [ErrorKind; 10] = [
        ErrorKind::Failed,
        ErrorKind::Usage,
        ErrorKind::NotFound,
        ErrorKind::Conflict,
        ErrorKind::Refused,
        ErrorKind::Config,
        ErrorKind::Unreachable,
        ErrorKind::Indeterminate,
        ErrorKind::NotBuilt,
        ErrorKind::Elsewhere,
    ];

    /// The process exit code for this kind.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::Failed => 1,
            ErrorKind::Usage => 2,
            ErrorKind::NotFound => 3,
            ErrorKind::Conflict => 4,
            ErrorKind::Refused => 5,
            ErrorKind::Config => 6,
            ErrorKind::Unreachable => 7,
            ErrorKind::Indeterminate => 8,
            ErrorKind::NotBuilt => 20,
            ErrorKind::Elsewhere => 21,
        }
    }

    /// A stable machine-readable name, used in `--json` error output.
    pub fn name(self) -> &'static str {
        match self {
            ErrorKind::Failed => "FAILED",
            ErrorKind::Usage => "USAGE",
            ErrorKind::NotFound => "NOT_FOUND",
            ErrorKind::Conflict => "CONFLICT",
            ErrorKind::Refused => "REFUSED",
            ErrorKind::Config => "CONFIG",
            ErrorKind::Unreachable => "UNREACHABLE",
            ErrorKind::Indeterminate => "INDETERMINATE",
            ErrorKind::NotBuilt => "NOT_BUILT",
            ErrorKind::Elsewhere => "ELSEWHERE",
        }
    }
}

/// An error with its kind and a message meant for the person or agent reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdError {
    /// What went wrong.
    pub kind: ErrorKind,
    /// What to tell the caller, including what to do next where that is known.
    pub message: String,
}

impl SdError {
    /// Build an error of `kind`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// A usage error.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, message)
    }

    /// A missing seed.
    pub fn not_found(id: &str) -> Self {
        Self::new(ErrorKind::NotFound, format!("no seed with id {id}"))
    }

    /// A lost compare-and-set.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conflict, message)
    }

    /// A refused write.
    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Refused, message)
    }

    /// A store or I/O failure.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Failed, message)
    }

    /// The process exit code.
    pub fn exit_code(&self) -> i32 {
        self.kind.exit_code()
    }
}

impl fmt::Display for SdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SdError {}

impl From<quipu::Error> for SdError {
    fn from(e: quipu::Error) -> Self {
        SdError::failed(format!("quipu: {e}"))
    }
}

/// Result alias for the core.
pub type Result<T> = std::result::Result<T, SdError>;
