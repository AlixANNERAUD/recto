//! The pages of the interface.

pub mod account;
pub mod administration;
pub mod auth;
pub mod jobs;
pub mod no_access;
pub mod offline;
pub mod print;
pub mod printers;
pub mod scan;
pub mod sign_in;

use leptos::prelude::*;

use crate::components::{
    error_text,
    ui::{Alert, Tone},
};

/// The message of a page whose data could not be loaded, such as CUPS being unreachable: the
/// page still renders around it.
pub(crate) fn failure_alert(error: &ServerFnError) -> impl IntoView + use<> {
    let message = error_text(error);
    view! { <Alert tone=Tone::Danger>{message}</Alert> }
}
