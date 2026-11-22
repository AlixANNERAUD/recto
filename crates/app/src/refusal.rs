//! Why a print or cancel form was refused. The server redirects back with the refusal's code in
//! the `error` query parameter (`/?error=too_large`), and the page shows its message. Only codes
//! travel in the address, so a crafted link cannot put arbitrary text on the page.

use crate::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    TooLarge,
    NoFile,
    EmptyFile,
    UnsupportedFormat,
    PrinterNotFound,
    NotAccepting,
    InvalidCopies,
    UnsupportedSides,
    UnsupportedColor,
    InvalidPageRanges,
    InvalidForm,
    JobNotFound,
    Unavailable,
    Refused,
    /// A hard quota rule has too little left; the rule's id travels next to the code.
    Quota,
    /// Quota rules count pages and recto could not count the document's.
    Uncountable,
    /// Quota rules count pages and recto was busy counting other documents.
    CountingBusy,
}

impl Refusal {
    const ALL: [Refusal; 17] = [
        Refusal::TooLarge,
        Refusal::NoFile,
        Refusal::EmptyFile,
        Refusal::UnsupportedFormat,
        Refusal::PrinterNotFound,
        Refusal::NotAccepting,
        Refusal::InvalidCopies,
        Refusal::UnsupportedSides,
        Refusal::UnsupportedColor,
        Refusal::InvalidPageRanges,
        Refusal::InvalidForm,
        Refusal::JobNotFound,
        Refusal::Unavailable,
        Refusal::Refused,
        Refusal::Quota,
        Refusal::Uncountable,
        Refusal::CountingBusy,
    ];

    /// The value of the `error` query parameter.
    pub fn code(self) -> &'static str {
        match self {
            Refusal::TooLarge => "too_large",
            Refusal::NoFile => "no_file",
            Refusal::EmptyFile => "empty_file",
            Refusal::UnsupportedFormat => "unsupported_format",
            Refusal::PrinterNotFound => "printer_not_found",
            Refusal::NotAccepting => "not_accepting",
            Refusal::InvalidCopies => "invalid_copies",
            Refusal::UnsupportedSides => "unsupported_sides",
            Refusal::UnsupportedColor => "unsupported_color",
            Refusal::InvalidPageRanges => "invalid_page_ranges",
            Refusal::InvalidForm => "invalid_form",
            Refusal::JobNotFound => "job_not_found",
            Refusal::Unavailable => "unavailable",
            Refusal::Refused => "refused",
            Refusal::Quota => "quota",
            Refusal::Uncountable => "uncountable",
            Refusal::CountingBusy => "counting_busy",
        }
    }

    pub fn from_code(code: &str) -> Option<Refusal> {
        Self::ALL.into_iter().find(|refusal| refusal.code() == code)
    }

    /// The message shown on the page, in the request's language.
    pub fn message(self) -> String {
        match self {
            Refusal::TooLarge => t!("print.errors.too_large"),
            Refusal::NoFile => t!("print.errors.no_file"),
            Refusal::EmptyFile => t!("print.errors.empty_file"),
            Refusal::UnsupportedFormat => t!("print.errors.unsupported_format"),
            Refusal::PrinterNotFound => t!("print.errors.printer_not_found"),
            Refusal::NotAccepting => t!("print.errors.not_accepting"),
            Refusal::InvalidCopies => t!("print.errors.invalid_copies"),
            Refusal::UnsupportedSides => t!("print.errors.unsupported_sides"),
            Refusal::UnsupportedColor => t!("print.errors.unsupported_color"),
            Refusal::InvalidPageRanges => t!("print.errors.invalid_page_ranges"),
            Refusal::InvalidForm => t!("print.errors.invalid_form"),
            Refusal::JobNotFound => t!("print.errors.job_not_found"),
            Refusal::Unavailable => t!("print.errors.unavailable"),
            Refusal::Refused => t!("print.errors.refused"),
            Refusal::Quota => t!("print.errors.quota"),
            Refusal::Uncountable => t!("print.errors.uncountable"),
            Refusal::CountingBusy => t!("print.errors.counting_busy"),
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::Refusal;

    #[test]
    fn codes_round_trip() {
        for refusal in Refusal::ALL {
            assert_eq!(Refusal::from_code(refusal.code()), Some(refusal));
            assert!(!refusal.message().is_empty());
        }
        assert_eq!(Refusal::from_code("<script>"), None);
    }
}
