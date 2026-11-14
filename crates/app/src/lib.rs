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

// Leptos views are deeply nested types; the print form's goes past the default limit.
#![recursion_limit = "256"]

// Every embedded locale must translate every message (strict): a missing key fails the build.
// Each request translates into its own locale (`locale = ...`, see [`i18n`]); the global locale
// of embedded-i18n stays unset, so concurrent requests cannot overwrite each other's language.
embedded_i18n::i18n!(
    "locales",
    default_locale = "en",
    available_locales = ["en"],
    strict = true
);

pub mod components;
pub mod display;
pub mod functions;
pub mod i18n;
pub mod islands;
pub mod layout;
pub mod messages;
pub mod pages;
pub mod refusal;
#[cfg(feature = "ssr")]
pub mod state;

use leptos::prelude::*;
use leptos_meta::{MetaTags, provide_meta_context};
use leptos_router::{
    SsrMode,
    components::{Route, Router, Routes},
    path,
};
use pages::{
    auth::{SignInErrorPage, SignedOutPage},
    jobs::JobsPage,
    no_access::NoAccessPage,
    print::PrintPage,
    printers::PrintersPage,
    scan::ScanPage,
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
                <meta name="color-scheme" content="light dark" />
                <link rel="stylesheet" id="leptos" href="/pkg/recto.css" />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
                <AutoReload options=options.clone() />
                <HydrationScripts options islands=true />
                <MetaTags />
            </head>
            <body class="bg-background text-foreground min-h-screen font-sans antialiased">
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
                <Route path=path!("/") view=PrintPage ssr=SsrMode::Async />
                <Route path=path!("/printers") view=PrintersPage ssr=SsrMode::Async />
                <Route path=path!("/jobs") view=JobsPage ssr=SsrMode::Async />
                <Route path=path!("/scan") view=ScanPage ssr=SsrMode::Async />
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
        <layout::Layout title=t!("not_found.heading").to_string()>
            <p>
                <a class="text-brand underline underline-offset-4" href="/">
                    {t!("not_found.home")}
                </a>
            </p>
        </layout::Layout>
    }
}
