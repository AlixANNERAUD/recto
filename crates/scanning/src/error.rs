use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Everything that can go wrong while scanning or storing a result. It is serializable because it
/// crosses the server-function boundary, so it carries plain strings rather than source errors.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum ScanError {
    #[error("scanner busy")]
    Busy,
    #[error("scanner not found: {0}")]
    NotFound(String),
    #[error("paper jam in the scanner")]
    Jammed,
    #[error("no documents in the feeder")]
    NoDocuments,
    #[error("scanner cover is open")]
    CoverOpen,
    #[error("unsupported by the scanner: {0}")]
    Unsupported(String),
    #[error("scanner error: {0}")]
    Device(String),
    /// One page would need more memory than recto allows ([`crate::MAX_PAGE_BYTES`]).
    #[error("the scan is too large")]
    TooLarge,
    #[error("cannot encode the scan: {0}")]
    Encoding(String),
    #[error("cannot write to {}: {reason}", path.display())]
    Storage { path: PathBuf, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_round_trip() {
        let e = ScanError::Storage {
            path: "/scans".into(),
            reason: "denied".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<ScanError>(&json).unwrap(), e);
        assert_eq!(e.to_string(), "cannot write to /scans: denied");
    }
}
