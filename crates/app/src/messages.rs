//! Messages the server sends outside pages (plain or JSON answers), and those of scan errors.
//! Translations resolve in this crate, which holds the locales, so the server asks here instead
//! of translating itself. Each takes the locale explicitly: outside a page there is no request
//! context to read it from (see [`crate::i18n::from_headers`]).

use embedded_i18n::{Locale, t};
#[cfg(feature = "ssr")]
use recto_domain::held::HeldState;
use recto_scanning::ScanError;

/// The message of the server function error answered to a call without a signed-in user. It is
/// a marker rather than text: the islands recognise it and say the session ended.
pub const SIGNED_OUT: &str = "recto:signed-out";

pub fn sign_in_required(locale: Locale) -> &'static str {
    t!("auth.sign_in_required", locale = locale)
}

/// Why a form or action was refused as cross-site. Most often recto was opened at another
/// address than its public URL (`origin`), such as its LAN address. `name` is the app's name.
pub fn cross_site_refused(locale: Locale, name: &str, origin: &str) -> String {
    format!(
        "{}",
        t!(
            "auth.cross_site_refused",
            name = name,
            origin = origin,
            locale = locale
        )
    )
}

pub fn unknown_theme(locale: Locale) -> &'static str {
    t!("theme.unknown", locale = locale)
}

pub fn scan_result_not_found(locale: Locale) -> &'static str {
    t!("scan.errors.result_not_found", locale = locale)
}

pub fn save_disabled(locale: Locale) -> &'static str {
    t!("scan.errors.save_disabled", locale = locale)
}

/// Why a usage export refused its range.
pub fn invalid_usage_dates(locale: Locale) -> &'static str {
    t!("administration.usage.invalid_dates", locale = locale)
}

/// Why a usage export failed: the database did.
pub fn usage_unavailable(locale: Locale) -> &'static str {
    t!("administration.usage.load_failed", locale = locale)
}

/// The `job-state-message` a device shows for a job it sent: where the job is in recto. `name`
/// is the app's name.
#[cfg(feature = "ssr")]
pub fn held_job(state: HeldState, name: &str, locale: Locale) -> String {
    match state {
        HeldState::Incoming => t!("ipp.incoming", locale = locale).to_owned(),
        HeldState::Held => format!("{}", t!("ipp.held", name = name, locale = locale)),
        HeldState::Releasing => format!("{}", t!("ipp.releasing", name = name, locale = locale)),
        HeldState::Released => format!("{}", t!("ipp.released", name = name, locale = locale)),
    }
}

/// The `job-state-message` of a job its owner prints directly that was refused, so it is held
/// instead; `reason` is why, a refusal's message.
#[cfg(feature = "ssr")]
pub fn direct_refused(reason: &str, name: &str, locale: Locale) -> String {
    format!(
        "{}",
        t!(
            "ipp.direct_refused",
            reason = reason,
            name = name,
            locale = locale
        )
    )
}

/// What went wrong while scanning or storing a scan, readable by the user. `name` is the app's
/// name.
pub fn scan_error(error: &ScanError, name: &str, locale: Locale) -> String {
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
        ScanError::TooLarge => format!(
            "{}",
            t!("scan.errors.too_large", name = name, locale = locale)
        ),
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
            scan_error(&error, "recto", crate::i18n::default_locale()),
            "Cannot write to /scans: permission denied"
        );
    }

    #[test]
    fn interruptions_and_refusals_are_translated() {
        let locale = crate::i18n::default_locale();
        assert_eq!(
            scan_error(&ScanError::Interrupted, "recto", locale),
            "The scan stopped unexpectedly. Try again."
        );
        assert!(
            cross_site_refused(locale, "Maison", "https://recto.example.org")
                .contains("Open Maison at https://recto.example.org")
        );
        assert!(
            scan_error(&ScanError::TooLarge, "Maison", locale).contains("too large for Maison")
        );
    }
}
