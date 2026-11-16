//! `/no-access`: the provider signed the user in, but the required claim was missing.

use leptos::prelude::*;

use super::auth::SignOutButton;
use crate::{branding::app_name, layout::Bare, tf};

#[component]
pub fn NoAccessPage() -> impl IntoView {
    view! {
        <Bare title=tf!("auth.no_access.heading", name = app_name())>
            <p>{tf!("auth.no_access.body", name = app_name())}</p>
            <SignOutButton />
        </Bare>
    }
}
