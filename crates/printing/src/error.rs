//! Errors of the printing service.

use thiserror::Error;

/// Why a print operation failed, in terms the interface can show.
#[derive(Clone, Debug, PartialEq, Eq, Error, serde::Serialize, serde::Deserialize)]
pub enum PrintError {
    /// CUPS could not be reached or answered something unusable.
    #[error("printing service unavailable: {0}")]
    Unavailable(String),
    #[error("printer not found: {0}")]
    PrinterNotFound(String),
    #[error("job {0} not found")]
    JobNotFound(i32),
    /// CUPS understood the request and declined it.
    #[error("printing service refused the request: {0}")]
    Refused(String),
    #[error("invalid page ranges: {0}")]
    InvalidPageRanges(String),
}
