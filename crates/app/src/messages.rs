//! Messages the server sends outside pages (plain or JSON answers), and those of scan errors.
//! Translations resolve in this crate, which holds the locales, so the server asks here instead
//! of translating itself. Each takes the locale explicitly: outside a page there is no request
//! context to read it from (see [`crate::i18n::from_headers`]).

use embedded_i18n::{Locale, t};
use recto_scanning::ScanError;

/// The message of the server function error answered to a call without a signed-in user. It is
/// a marker rather than text: the islands recognise it and say the session ended.
pub const SIGNED_OUT: &str = "recto:signed-out";

pub fn sign_in_required(locale: Locale) -> &'static str {
    t!("auth.sign_in_required", locale = locale)
}

/// Why a form or action was refused as cross-site. Most often recto was opened at another
/// address than its public URL (`origin`), such as its LAN address.
pub fn cross_site_refused(locale: Locale, origin: &str) -> String {
    format!(
        "{}",
        t!("auth.cross_site_refused", origin = origin, locale = locale)
    )
}

pub fn scan_result_not_found(locale: Locale) -> &'static str {
    t!("scan.errors.result_not_found", locale = locale)
}

pub fn save_disabled(locale: Locale) -> &'static str {
    t!("scan.errors.save_disabled", locale = locale)
}

/// What went wrong while scanning or storing a scan, readable by the user.
pub fn scan_error(error: &ScanError, locale: Locale) -> String {
    match error {
        ScanError::Busy => t!("scan.errors.busy", locale = locale).to_owned(),
        ScanError::NotFound(_) => t!("scan.errors.not_found", locale = locale).to_owned(),
        ScanError::Jammed => t!("scan.errors.jammed", locale = locale).to_owned(),
        ScanError::NoDocuments => t!("scan.errors.no_documents", locale = locale).to_owned(),
        ScanError::CoverOpen => t!("scan.errors.cover_open", locale = locale).to_owned(),
        ScanError::Unsupported(what) => format!(
            "{}",
            t!("scan.errors.unsupported", what = what, locale = locale)
        ),
        ScanError::Device(reason) => format!(
            "{}",
            t!("scan.errors.device", reason = reason, locale = locale)
        ),
        ScanError::TooLarge => t!("scan.errors.too_large", locale = locale).to_owned(),
        ScanError::Interrupted => t!("scan.errors.interrupted", locale = locale).to_owned(),
        ScanError::Encoding(reason) => format!(
            "{}",
            t!("scan.errors.encoding", reason = reason, locale = locale)
        ),
        ScanError::Storage { path, reason } => format!(
            "{}",
            t!(
                "scan.errors.storage",
                path = path.display(),
                reason = reason,
                locale = locale
            )
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_errors_name_the_folder() {
        let error = ScanError::Storage {
            path: "/scans".into(),
            reason: "permission denied".into(),
        };
        assert_eq!(
            scan_error(&error, crate::i18n::default_locale()),
            "Cannot write to /scans: permission denied"
        );
    }

    #[test]
    fn interruptions_and_refusals_are_translated() {
        let locale = crate::i18n::default_locale();
        assert_eq!(
            scan_error(&ScanError::Interrupted, locale),
            "The scan stopped unexpectedly. Try again."
        );
        assert!(
            cross_site_refused(locale, "https://recto.example.org")
                .contains("Open recto at https://recto.example.org")
        );
    }
}
