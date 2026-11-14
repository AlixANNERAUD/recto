//! `/no-access`: the provider signed the user in, but the required claim was missing.

use leptos::prelude::*;

use super::auth::SignOutButton;
use crate::{layout::Bare, t};

#[component]
pub fn NoAccessPage() -> impl IntoView {
    view! {
        <Bare title=t!("auth.no_access.heading")>
            <p>{t!("auth.no_access.body")}</p>
            <SignOutButton />
        </Bare>
    }
}
