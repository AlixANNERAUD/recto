//! `/jobs`: print jobs, with Cancel for active ones. The print form lands here with
//! `?submitted=<id>`, and `&over=<rule id>,<rule id>` when the job went over soft quota rules;
//! a refused cancel with `?error=<code>`.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::failure_alert;
use crate::{
    components::ui::{Alert, Tone},
    currency::Currency,
    display::unix_now,
    functions::{Allowances, list_jobs, my_quotas},
    islands::jobs::JobList,
    layout::Layout,
    refusal::Refusal,
    t, tf,
};

/// The currency costs read in.
#[cfg(feature = "ssr")]
fn currency() -> Currency {
    crate::state::require_user()
        .ok()
        .and_then(|_| crate::state::app_state())
        .map_or_else(|| Currency::new("EUR"), |state| state.currency)
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
fn currency() -> Currency {
    Currency::new("EUR")
}

#[component]
pub fn JobsPage() -> impl IntoView {
    let query = use_query_map();
    let (notice, over) = {
        let query = query.read_untracked();
        if let Some(refusal) = query
            .get("error")
            .and_then(|code| Refusal::from_code(&code))
        {
            (
                Some(view! { <Alert tone=Tone::Danger>{refusal.message()}</Alert> }.into_any()),
                Vec::new(),
            )
        } else {
            // Only numbers are read, so a crafted link cannot put text on the page.
            let submitted = query.get("submitted").and_then(|id| id.parse::<i32>().ok());
            let notice = submitted.map(|id| {
                view! { <Alert tone=Tone::Success>{tf!("jobs.submitted", id = id)}</Alert> }
                    .into_any()
            });
            let over: Vec<i64> = submitted
                .and(query.get("over"))
                .map(|ids| {
                    ids.split(',')
                        .filter_map(|id| id.trim().parse().ok())
                        .collect()
                })
                .unwrap_or_default();
            (notice, over)
        }
    };
    let warnings = (!over.is_empty()).then(|| {
        let quotas = OnceResource::new(my_quotas());
        view! {
            <Suspense>
                {move || {
                    let over = over.clone();
                    Suspend::new(async move {
                        let quotas = quotas.await.ok();
                        over_messages(quotas.as_ref(), &over)
                            .into_iter()
                            .map(|message| view! { <Alert tone=Tone::Warning>{message}</Alert> })
                            .collect_view()
                    })
                }}
            </Suspense>
        }
    });
    let jobs = OnceResource::new(list_jobs());
    let currency = currency();
    view! {
        <Layout title=t!("jobs.title")>
            {notice}
            {warnings}
            <Suspense>
                {move || {
                    let currency = currency.clone();
                    Suspend::new(async move {
                        match jobs.await {
                            Ok(jobs) => view! { <JobList jobs now=unix_now() currency /> }.into_any(),
                            Err(error) => failure_alert(&error).into_any(),
                        }
                    })
                }}
            </Suspense>
        </Layout>
    }
}

/// One warning per soft rule the job went over, naming it and what is left; one plain warning
/// when the rules cannot be read or no longer apply.
fn over_messages(quotas: Option<&Allowances>, over: &[i64]) -> Vec<String> {
    let named: Vec<String> = quotas
        .map(|quotas| {
            over.iter()
                .filter_map(|rule| {
                    let allowance = quotas
                        .allowances
                        .iter()
                        .find(|allowance| allowance.rule == *rule)?;
                    Some(tf!(
                        "jobs.over",
                        rule = allowance.name.clone(),
                        left = allowance.left(&quotas.currency)
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    if named.is_empty() {
        vec![t!("jobs.over_unknown").to_owned()]
    } else {
        named
    }
}
