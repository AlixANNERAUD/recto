//! Building blocks shared by the pages and the islands.

pub mod icons;
pub mod printer;
pub mod ui;

use leptos::prelude::*;

use crate::t;

/// The message of a server function's error, for the page. Server functions put a translated,
/// readable message in their errors; anything else is the request failing on the way.
pub fn error_text(error: &ServerFnError) -> String {
    match error {
        ServerFnError::ServerError(message) => message.clone(),
        _ => t!("common.network_error").to_string(),
    }
}
