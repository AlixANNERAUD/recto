//! Print models and the CUPS/IPP client.
//!
//! The models depend only on `serde` so the interface can use them in the browser. Anything that
//! touches `ipp` or `bytes` is behind the `service` feature.

mod error;
mod format;
mod model;
mod options;
pub mod printer;
mod status;

#[cfg(feature = "service")]
mod admin;
#[cfg(feature = "service")]
pub mod count;
#[cfg(feature = "service")]
mod cups;
#[cfg(feature = "service")]
mod service;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(feature = "service")]
pub use admin::{CupsAdmin, PrinterAdmin};
#[cfg(feature = "service")]
pub use cups::Cups;
pub use error::{AdminError, PrintError};
pub use format::DocumentFormat;
#[cfg(feature = "service")]
pub use model::PrintRequest;
pub use model::{Capabilities, Driver, DriverInfo, Job, JobStatus, NewPrinter, Printer};
pub use options::{ColorMode, PageRanges, PrintOptions, Sides};
#[cfg(feature = "service")]
pub use service::PrintService;
pub use status::{JobState, PrinterState, ReasonKind, Severity, StateReason, Supply, supplies};
