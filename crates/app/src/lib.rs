//! Web interface of recto, rendered on the server. Pages are complete HTML and actions are plain
//! forms, so print and cancel work without JavaScript; islands add the scan page and live
//! refresh.
//!
//! The crate is compiled twice. With `ssr`, for the server: pages render to HTML. With
//! `hydrate`, for the browser bundle built by the `frontend` crate: only islands run. The two
//! features are never enabled in the same build.
//!
//! Every string goes through [`t!`], [`tf!`] or [`tp!`] ([`i18n`]), with keys in
//! `locales/en.json`.

// Every embedded locale must translate every message (strict): a missing key fails the build.
// Each request translates into its own locale (`locale = ...`, see [`i18n`]); the global locale
// of embedded-i18n stays unset, so concurrent requests cannot overwrite each other's language.
embedded_i18n::i18n!(
    "locales",
    default_locale = "en",
    available_locales = ["en"],
    strict = true
);

pub mod i18n;
pub mod messages;
pub mod pages;
pub mod refusal;
#[cfg(feature = "ssr")]
pub mod state;

use leptos::prelude::*;
use leptos_meta::{MetaTags, Title, provide_meta_context};
use leptos_router::{
    SsrMode,
    components::{Route, Router, Routes},
    path,
};
use pages::{
    auth::{NoAccessPage, SignInErrorPage, SignedOutPage},
    home::HomePage,
};

/// The HTML document around every page. Its `lang` attribute is the request's language, which
/// islands read back in the browser.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    let language = i18n::tag(i18n::locale());
    view! {
        <!DOCTYPE html>
        <html lang=language>
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <link rel="stylesheet" id="leptos" href="/pkg/recto.css" />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
                <AutoReload options=options.clone() />
                <HydrationScripts options islands=true />
                <MetaTags />
            </head>
            <body class="min-h-screen antialiased">
                <App />
            </body>
        </html>
    }
}

/// The router: every page of the interface.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    i18n::provide_locale();
    view! {
        <Router>
            <Routes fallback=NotFound>
                <Route path=path!("/") view=HomePage ssr=SsrMode::Async />
                <Route path=path!("/no-access") view=NoAccessPage />
                <Route path=path!("/auth/error") view=SignInErrorPage />
                <Route path=path!("/auth/signed-out") view=SignedOutPage />
            </Routes>
        </Router>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    #[cfg(feature = "ssr")]
    if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
        response.set_status(axum::http::StatusCode::NOT_FOUND);
    }
    view! {
        <Title text=t!("not_found.title") />
        <main class="mx-auto max-w-xl p-8">
            <h1 class="text-2xl font-semibold">{t!("not_found.heading")}</h1>
        </main>
    }
}
