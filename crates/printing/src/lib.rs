//! Print models and the CUPS/IPP client.
//!
//! The models depend only on `serde` so the interface can use them in the browser. Anything that
//! touches `ipp` or `bytes` is behind the `service` feature.

mod error;
mod format;
mod model;
mod options;
mod status;

pub use error::PrintError;
pub use format::DocumentFormat;
#[cfg(feature = "service")]
pub use model::PrintRequest;
pub use model::{Capabilities, Job, Printer};
pub use options::{ColorMode, PageRanges, PrintOptions, Sides};
pub use status::{JobState, PrinterState, ReasonKind, Severity, StateReason, Supply, supplies};
