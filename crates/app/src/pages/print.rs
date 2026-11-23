//! `/`: the print form, the reason the last one was refused (`?error=<code>`, with
//! `&rule=<id>` for a quota), the status of the chosen printer, and what the user has left of
//! their print quotas.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::{failure_alert, refusal_alert};
use crate::{
    components::ui::{Card, Empty},
    functions::{Allowances, print_choices},
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
    let (refusal, rule) = {
        let query = query.read_untracked();
        (
            query
                .get("error")
                .and_then(|code| Refusal::from_code(&code)),
            query.get("rule").and_then(|id| id.parse::<i64>().ok()),
        )
    };
    let max_upload_size = upload_limit();
    // One listing of the printers serves the form and the quotas.
    let choices = OnceResource::new(print_choices());
    let refusal = refusal.map(|refusal| {
        refusal_alert(
            refusal,
            rule,
            || choices,
            |choices| choices.ok().and_then(|choices| choices.quotas),
        )
    });
    view! {
        <Layout title=t!("print.title")>
            {refusal}
            <Suspense>
                {move || Suspend::new(async move {
                    match choices.await.map(|choices| choices.printers) {
                        Ok(printers) if printers.is_empty() => {
                            view! { <Empty title=t!("printers.empty") body=t!("printers.empty_body") /> }
                                .into_any()
                        }
                        Ok(printers) => view! { <PrintForm printers max_upload_size /> }.into_any(),
                        Err(error) => failure_alert(&error).into_any(),
                    }
                })}
            </Suspense>
            <Suspense>
                {move || Suspend::new(async move {
                    // Without rules there is nothing to show. A failure to read them shows
                    // nothing either: printing refuses on its own when quotas cannot be checked.
                    choices
                        .await
                        .ok()
                        .and_then(|choices| choices.quotas)
                        .filter(|quotas| !quotas.allowances.is_empty())
                        .map(|quotas| view! { <Left quotas /> })
                })}
            </Suspense>
        </Layout>
    }
}

/// What the user has left of each rule that applies to them.
#[component]
fn Left(quotas: Allowances) -> impl IntoView {
    let currency = quotas.currency;
    let lines = quotas
        .allowances
        .into_iter()
        .map(|allowance| {
            let left = allowance.left(&currency);
            view! {
                <li class="flex flex-wrap gap-x-2">
                    <span class="font-medium">{allowance.name}</span>
                    <span class="text-muted-foreground">{left}</span>
                </li>
            }
        })
        .collect_view();
    view! {
        <Card title=t!("print.left.title")>
            <ul class="grid gap-2 text-sm">{lines}</ul>
        </Card>
    }
}
