//! `/`: the print form, the reason the last one was refused (`?error=<code>`, with
//! `&rule=<id>` for a quota), the status of the chosen printer, and what the user has left of
//! their print quotas.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::failure_alert;
use crate::{
    components::ui::{Alert, Card, Empty, Tone},
    functions::{Allowances, list_printers, my_quotas},
    islands::print_form::PrintForm,
    layout::Layout,
    refusal::Refusal,
    t, tf,
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
    let printers = OnceResource::new(list_printers());
    let quotas = OnceResource::new(my_quotas());
    let refusal = refusal.map(|refusal| {
        view! {
            <Suspense>
                {move || Suspend::new(async move {
                    // A quota refusal names the rule and what is left of it, read now; only
                    // its id travels in the address.
                    let message = match (refusal, rule) {
                        (Refusal::Quota, Some(rule)) => quotas
                            .await
                            .ok()
                            .and_then(|quotas| quota_refusal(&quotas, rule))
                            .unwrap_or_else(|| refusal.message()),
                        _ => refusal.message(),
                    };
                    view! { <Alert tone=Tone::Danger>{message}</Alert> }
                })}
            </Suspense>
        }
    });
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
            <Suspense>
                {move || Suspend::new(async move {
                    // Without rules there is nothing to show. A failure to read them shows
                    // nothing either: printing refuses on its own when quotas cannot be checked.
                    quotas
                        .await
                        .ok()
                        .filter(|quotas| !quotas.allowances.is_empty())
                        .map(|quotas| view! { <Left quotas /> })
                })}
            </Suspense>
        </Layout>
    }
}

/// The message of a job refused by the rule `rule`, when it still applies to the user.
fn quota_refusal(quotas: &Allowances, rule: i64) -> Option<String> {
    let allowance = quotas
        .allowances
        .iter()
        .find(|allowance| allowance.rule == rule)?;
    Some(tf!(
        "print.errors.quota_rule",
        rule = allowance.name.clone(),
        left = allowance.left(&quotas.currency)
    ))
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
