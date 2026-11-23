//! recto as a driverless IPP printer: what it tells devices about itself and their jobs.
//!
//! Pure: no database, no network. The server's IPP routes authenticate, read the body and keep
//! the jobs; this module decodes their requests and encodes the answers. [`Media`] and
//! [`Requested`] only need `serde`, so the interface can show what a device asked for.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{ColorMode, PageRanges, Sides};

#[cfg(feature = "service")]
mod attributes;
#[cfg(feature = "service")]
mod operations;
#[cfg(all(test, feature = "service"))]
mod tests;

#[cfg(feature = "service")]
pub use attributes::{
    DOCUMENT_FORMATS, JobView, JobViewState, PrinterUri, REQUIRED_PRINTER_ATTRIBUTES,
    VirtualPrinter,
};
#[cfg(feature = "service")]
pub use operations::{
    GET_PRINTER_ATTRIBUTES, Header, Incoming, Operation, TOO_MANY_JOBS, WhichJobs, decode, header,
    refusal, response,
};

/// The paper sizes the queue offers. The document stays page-size independent until it is
/// released to a real printer, so these only tell the device how to lay it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Media {
    A4,
    Letter,
    Legal,
    A3,
    A5,
}

impl Media {
    pub const ALL: [Media; 5] = [Media::A4, Media::Letter, Media::Legal, Media::A3, Media::A5];

    /// The PWG self-describing media name.
    pub fn keyword(self) -> &'static str {
        match self {
            Media::A4 => "iso_a4_210x297mm",
            Media::Letter => "na_letter_8.5x11in",
            Media::Legal => "na_legal_8.5x14in",
            Media::A3 => "iso_a3_297x420mm",
            Media::A5 => "iso_a5_148x210mm",
        }
    }

    /// Width and height in hundredths of a millimetre, the unit of `media-size`.
    pub fn size(self) -> (i32, i32) {
        match self {
            Media::A4 => (21000, 29700),
            Media::Letter => (21590, 27940),
            Media::Legal => (21590, 35560),
            Media::A3 => (29700, 42000),
            Media::A5 => (14800, 21000),
        }
    }
}

/// A media name that is not one of the five the queue offers.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown media {0:?}: expected iso_a4_210x297mm, na_letter_8.5x11in, na_legal_8.5x14in, \
     iso_a3_297x420mm or iso_a5_148x210mm"
)]
pub struct UnknownMedia(pub String);

impl FromStr for Media {
    type Err = UnknownMedia;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Media::ALL
            .into_iter()
            .find(|media| media.keyword() == input)
            .ok_or_else(|| UnknownMedia(input.to_owned()))
    }
}

/// The options a device asked for, kept with a held job to pre-fill its release. Values the
/// queue does not offer are dropped rather than guessed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requested {
    pub copies: Option<u32>,
    pub sides: Option<Sides>,
    pub color_mode: Option<ColorMode>,
    pub page_ranges: Option<PageRanges>,
    pub media: Option<Media>,
}
