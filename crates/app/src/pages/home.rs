//! `/`: the print page. For now it shows why the last print form was refused and whether CUPS
//! answers; the form itself comes with the rest of the interface.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use crate::{refusal::Refusal, t};

#[component]
pub fn HomePage() -> impl IntoView {
    let query = use_query_map();
    let refusal = move || {
        query
            .read()
            .get("error")
            .and_then(|code| Refusal::from_code(&code))
            .map(|refusal| view! { <p role="alert">{refusal.message()}</p> })
    };
    let banner = OnceResource::new(unavailable_banner());
    view! {
        <Title text=t!("app.name") />
        <main class="mx-auto max-w-xl p-8">
            <h1 class="text-2xl font-semibold">{t!("app.name")}</h1>
            <p>{t!("app.tagline")}</p>
            {refusal}
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    banner.await.map(|message| view! { <p role="alert">{message}</p> })
                })}
            </Suspense>
        </main>
    }
}

/// `Printing service unavailable: <reason>` when CUPS does not answer, so the page renders
/// with an explanation rather than failing.
#[cfg(feature = "ssr")]
fn unavailable_banner() -> impl Future<Output = Option<String>> + Send + 'static {
    // Contexts are read before the future runs: they belong to the rendering owner.
    let state = crate::state::require_user()
        .ok()
        .and_then(|_| crate::state::app_state());
    let locale = crate::i18n::locale();
    async move {
        let reason = match state?.print.printers().await {
            Ok(_) => return None,
            Err(recto_printing::PrintError::Unavailable(reason)) => reason,
            Err(error) => error.to_string(),
        };
        Some(format!(
            "{}",
            embedded_i18n::t!("print.unavailable_banner", reason = reason, locale = locale)
        ))
    }
}

/// Pages other than islands never run in the browser.
#[cfg(not(feature = "ssr"))]
async fn unavailable_banner() -> Option<String> {
    None
}
