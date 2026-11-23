//! The HTTP routes besides the pages, which the interface crate renders.

pub mod administration;
pub mod auth;
mod body;
pub mod devices;
pub mod escl;
pub mod held;
pub mod ipp;
pub mod password;
pub mod print;
pub mod pwa;
pub mod scan;
mod usage;

use axum::{
    body::Body,
    http::{Request, StatusCode, request::Parts},
    response::Response,
};
use leptos::prelude::provide_context;
use recto_app::{pages::sign_in::SignInMethods, shell, state::AppState};

/// Renders the interface's page at the request's path, as the page routes do, with `context`
/// provided next to the usual one. Forms use it to answer a refused post with their page and
/// what went wrong, rather than a redirect that could only carry a code. `status` replaces 200;
/// a page that sets its own status (not found) keeps it.
pub(crate) async fn render(
    state: &AppState,
    methods: SignInMethods,
    parts: Parts,
    status: StatusCode,
    context: impl Fn() + Clone + Send + Sync + 'static,
) -> Response {
    let options = state.leptos_options.clone();
    let state = state.clone();
    let handler = leptos_axum::render_app_async_with_context(
        move || {
            provide_context(state.clone());
            provide_context(methods);
            context();
        },
        move || shell(options.clone()),
    );
    let mut response = handler(Request::from_parts(parts, Body::empty())).await;
    if response.status() == StatusCode::OK {
        *response.status_mut() = status;
    }
    response
}
