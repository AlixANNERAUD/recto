//! Why a print, cancel or release form was refused. The server redirects back with the refusal's code in
//! the `error` query parameter (`/?error=too_large`), and the page shows its message. Only codes
//! travel in the address, so a crafted link cannot put arbitrary text on the page.

use crate::{coded::coded, i18n::Locale};

coded! {
    pub enum Refusal {
        TooLarge = "too_large",
        NoFile = "no_file",
        EmptyFile = "empty_file",
        UnsupportedFormat = "unsupported_format",
        PrinterNotFound = "printer_not_found",
        NotAccepting = "not_accepting",
        InvalidCopies = "invalid_copies",
        UnsupportedSides = "unsupported_sides",
        UnsupportedColor = "unsupported_color",
        InvalidPageRanges = "invalid_page_ranges",
        InvalidForm = "invalid_form",
        JobNotFound = "job_not_found",
        Unavailable = "unavailable",
        Refused = "refused",
        /// The print request may have reached CUPS, which did not confirm it: the job may print.
        Unconfirmed = "unconfirmed",
        /// A hard quota rule has too little left; the rule's id travels next to the code.
        Quota = "quota",
        /// Quota rules count pages and recto could not count the document's.
        Uncountable = "uncountable",
        /// Quota rules count pages and recto was busy counting other documents.
        CountingBusy = "counting_busy",
        /// A held job is no longer waiting: released, deleted or expired.
        HeldGone = "held_gone",
        /// The user is already sending as many documents as they may at once.
        UploadsBusy = "uploads_busy",
        /// The document did not arrive in time.
        TooSlow = "too_slow",
    }
}

impl Refusal {
    /// The message shown on the page, in the request's language.
    pub fn message(self) -> String {
        self.message_in(crate::i18n::locale())
    }

    /// The message in `locale`, for answers rendered outside a page, such as IPP's.
    pub fn message_in(self, locale: Locale) -> String {
        match self {
            Refusal::TooLarge => embedded_i18n::t!("print.errors.too_large", locale = locale),
            Refusal::NoFile => embedded_i18n::t!("print.errors.no_file", locale = locale),
            Refusal::EmptyFile => embedded_i18n::t!("print.errors.empty_file", locale = locale),
            Refusal::UnsupportedFormat => {
                embedded_i18n::t!("print.errors.unsupported_format", locale = locale)
            }
            Refusal::PrinterNotFound => {
                embedded_i18n::t!("print.errors.printer_not_found", locale = locale)
            }
            Refusal::NotAccepting => {
                embedded_i18n::t!("print.errors.not_accepting", locale = locale)
            }
            Refusal::InvalidCopies => {
                embedded_i18n::t!("print.errors.invalid_copies", locale = locale)
            }
            Refusal::UnsupportedSides => {
                embedded_i18n::t!("print.errors.unsupported_sides", locale = locale)
            }
            Refusal::UnsupportedColor => {
                embedded_i18n::t!("print.errors.unsupported_color", locale = locale)
            }
            Refusal::InvalidPageRanges => {
                embedded_i18n::t!("print.errors.invalid_page_ranges", locale = locale)
            }
            Refusal::InvalidForm => embedded_i18n::t!("print.errors.invalid_form", locale = locale),
            Refusal::JobNotFound => {
                embedded_i18n::t!("print.errors.job_not_found", locale = locale)
            }
            Refusal::Unavailable => embedded_i18n::t!("print.errors.unavailable", locale = locale),
            Refusal::Refused => embedded_i18n::t!("print.errors.refused", locale = locale),
            Refusal::Unconfirmed => embedded_i18n::t!("print.errors.unconfirmed", locale = locale),
            Refusal::Quota => embedded_i18n::t!("print.errors.quota", locale = locale),
            Refusal::Uncountable => embedded_i18n::t!("print.errors.uncountable", locale = locale),
            Refusal::CountingBusy => {
                embedded_i18n::t!("print.errors.counting_busy", locale = locale)
            }
            Refusal::HeldGone => embedded_i18n::t!("print.errors.held_gone", locale = locale),
            Refusal::UploadsBusy => {
                embedded_i18n::t!("print.errors.uploads_busy", locale = locale)
            }
            Refusal::TooSlow => embedded_i18n::t!("print.errors.too_slow", locale = locale),
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::Refusal;

    #[test]
    fn codes_round_trip() {
        for &refusal in Refusal::ALL {
            assert_eq!(Refusal::from_code(refusal.code()), Some(refusal));
            assert!(!refusal.message().is_empty());
        }
        assert_eq!(Refusal::from_code("<script>"), None);
    }
}
