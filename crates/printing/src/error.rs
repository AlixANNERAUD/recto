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

/// Why a CUPS administration operation failed, in terms the interface can show.
#[derive(Clone, Debug, PartialEq, Eq, Error, serde::Serialize, serde::Deserialize)]
pub enum AdminError {
    /// CUPS does not let recto administer it (HTTP 401, 403 or 426): a setup problem, not
    /// something the person can fix from the page.
    #[error("CUPS refused administration by recto")]
    Refused,
    /// CUPS could not reach the printer at the given address, with its own explanation.
    #[error("printer unreachable: {0}")]
    Unreachable(String),
    /// CUPS understood the request and declined it, with its own explanation.
    #[error("CUPS rejected the request: {0}")]
    Rejected(String),
    /// CUPS could not be asked: the socket is missing, an I/O error or a timeout.
    #[error("CUPS administration unavailable: {0}")]
    Unavailable(String),
}
