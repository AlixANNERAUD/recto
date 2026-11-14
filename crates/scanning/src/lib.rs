//! Scan models and the SANE client.
//!
//! The models depend only on `serde`, `thiserror` and `uuid` so the interface can use them in the
//! browser. Anything that touches `image`, the clock or the filesystem is behind the `service`
//! feature.

mod area;
mod capabilities;
mod error;
mod model;

#[cfg(feature = "service")]
mod encode;
#[cfg(feature = "service")]
mod pdf;
#[cfg(feature = "sane")]
mod sane;
#[cfg(feature = "service")]
mod save;
#[cfg(feature = "service")]
mod service;
#[cfg(feature = "service")]
mod store;
#[cfg(test)]
mod test_support;
#[cfg(any(test, feature = "testing"))]
mod testing;

pub use area::{MAX_PAGE_BYTES, ScanSettings, Selection, check_page_bytes, preview_settings};
pub use capabilities::{OptionConstraint, OptionInfo, OptionUnit, capabilities};
#[cfg(feature = "service")]
pub use encode::{ScannedPage, assemble};
pub use error::ScanError;
pub use model::{Area, OutputFormat, ScanMode, ScannerCapabilities, ScannerInfo, Source};
#[cfg(feature = "sane")]
pub use sane::SaneScanners;
#[cfg(feature = "service")]
pub use save::save_to_folder;
#[cfg(feature = "service")]
pub use service::{ScanRequest, ScannerService};
#[cfg(feature = "service")]
pub use store::{ResultStore, ScanResult};
#[cfg(any(test, feature = "testing"))]
pub use testing::MockScannerService;
