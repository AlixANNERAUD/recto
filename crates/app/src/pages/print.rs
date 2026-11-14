//! `/`: the print form, the reason the last one was refused (`?error=<code>`), and the status
//! of the chosen printer.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::failure_alert;
use crate::{
    components::ui::{Alert, Empty, Tone},
    functions::list_printers,
    islands::print_form::PrintForm,
    layout::Layout,
    refusal::Refusal,
    t,
};

/// The largest accepted upload, which the form states and checks before sending.
#[cfg(feature = "ssr")]
fn upload_limit() -> usize {
    crate::state::require_user()
        .ok()
        .and_then(|_| crate::state::app_state())
        .map_or(0, |state| state.max_upload_size)
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
fn upload_limit() -> usize {
    0
}

#[component]
pub fn PrintPage() -> impl IntoView {
    let query = use_query_map();
    let refusal = query
        .read_untracked()
        .get("error")
        .and_then(|code| Refusal::from_code(&code))
        .map(|refusal| view! { <Alert tone=Tone::Danger>{refusal.message()}</Alert> });
    let max_upload_size = upload_limit();
    let printers = OnceResource::new(list_printers());
    view! {
        <Layout title=t!("print.title")>
            {refusal}
            <Suspense>
                {move || Suspend::new(async move {
                    match printers.await {
                        Ok(printers) if printers.is_empty() => {
                            view! { <Empty title=t!("printers.empty") body=t!("printers.empty_body") /> }
                                .into_any()
                        }
                        Ok(printers) => view! { <PrintForm printers max_upload_size /> }.into_any(),
                        Err(error) => failure_alert(&error).into_any(),
                    }
                })}
            </Suspense>
        </Layout>
    }
}
