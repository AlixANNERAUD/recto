//! `/offline`: what the service worker shows when the server cannot be reached.

use leptos::prelude::*;

use crate::{branding::app_name, layout::Bare, t, tf};

#[component]
pub fn OfflinePage() -> impl IntoView {
    view! {
        <Bare title=t!("offline.heading").to_string() switch=false>
            <p>{tf!("offline.body", name = app_name())}</p>
            <p>
                <a class="text-brand underline underline-offset-4" href="/">
                    {t!("offline.retry")}
                </a>
            </p>
        </Bare>
    }
}
