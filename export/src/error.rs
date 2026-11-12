use std::path::PathBuf;

use crate::Format;

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong while exporting, importing or driving a USB
/// stick.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{0} does not exist")]
    NotFound(PathBuf),

    #[error("{0} is outside the writing folder")]
    OutsideWritingRoot(PathBuf),

    #[error("{0} is not a Markdown or text file")]
    UnsupportedSource(PathBuf),

    #[error("{0} is neither a file nor a folder")]
    InvalidSelection(PathBuf),

    #[error("there is nothing to export in {0}")]
    NothingToExport(PathBuf),

    #[error("destination {0} is not a directory")]
    InvalidDestination(PathBuf),

    #[error("{tool} is not installed; it is needed to write {format}")]
    MissingTool { tool: &'static str, format: Format },

    #[error("{tool} failed on {path} ({status}): {stderr}")]
    ToolFailed {
        tool: &'static str,
        path: PathBuf,
        status: String,
        stderr: String,
    },

    #[error("{tool} did not finish {path} within {seconds} seconds")]
    Timeout {
        tool: &'static str,
        path: PathBuf,
        seconds: u64,
    },

    #[error("{0} is empty after conversion")]
    EmptyOutput(PathBuf),

    #[error("drive id {0:?} is not a D-Bus object path")]
    InvalidDriveId(String),

    #[error("drive error: {0}")]
    Drive(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<zbus::Error> for Error {
    fn from(error: zbus::Error) -> Self {
        Self::Drive(error.to_string())
    }
}
