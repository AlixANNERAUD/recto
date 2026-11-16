//! Building blocks shared by the pages and the islands.

pub mod icons;
pub mod printer;
pub mod ui;

use leptos::prelude::*;

use crate::{branding::app_name, messages::SIGNED_OUT, t, tf};

/// How a server function call failed, as far as the browser can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The session ended (it lapsed, or recto restarted): only signing in again helps.
    SignedOut,
    /// recto refused, with a translated message.
    Refused(String),
    /// No answer from recto: the network failed, or something between the browser and recto
    /// answered instead, such as a reverse proxy's 502 or 504 page. recto encodes all its own
    /// errors, so anything it cannot decode came from elsewhere.
    Connection,
}

pub fn failure(error: &ServerFnError) -> Failure {
    match error {
        ServerFnError::ServerError(message) if message == SIGNED_OUT => Failure::SignedOut,
        ServerFnError::ServerError(message) => Failure::Refused(message.clone()),
        _ => Failure::Connection,
    }
}

/// The message of a server function's error, for the page.
pub fn error_text(error: &ServerFnError) -> String {
    match failure(error) {
        Failure::SignedOut => t!("auth.signed_out_notice").to_string(),
        Failure::Refused(message) => message,
        Failure::Connection => tf!("common.network_error", name = app_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_told_apart() {
        let signed_out = ServerFnError::ServerError(SIGNED_OUT.into());
        assert_eq!(failure(&signed_out), Failure::SignedOut);
        assert!(error_text(&signed_out).contains("signed out"));
        let refused = ServerFnError::new("Scanner not found.");
        assert_eq!(error_text(&refused), "Scanner not found.");
        // A proxy's error page does not decode as a server function error.
        let proxy = ServerFnError::Deserialization("missing delimiter".into());
        assert_eq!(failure(&proxy), Failure::Connection);
        assert_eq!(
            failure(&ServerFnError::Request("timeout".into())),
            Failure::Connection
        );
    }
}
