//! `/jobs`: print jobs, with Cancel for active ones. The print form lands here with
//! `?submitted=<id>`, a refused cancel with `?error=<code>`.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::failure_alert;
use crate::{
    components::ui::{Alert, Tone},
    display::unix_now,
    functions::list_jobs,
    islands::jobs::JobList,
    layout::Layout,
    refusal::Refusal,
    t, tf,
};

#[component]
pub fn JobsPage() -> impl IntoView {
    let query = use_query_map();
    let notice = {
        let query = query.read_untracked();
        if let Some(refusal) = query
            .get("error")
            .and_then(|code| Refusal::from_code(&code))
        {
            Some(view! { <Alert tone=Tone::Danger>{refusal.message()}</Alert> }.into_any())
        } else {
            // Only a number is shown, so a crafted link cannot put text on the page.
            query
                .get("submitted")
                .and_then(|id| id.parse::<i32>().ok())
                .map(|id| {
                    view! { <Alert tone=Tone::Success>{tf!("jobs.submitted", id = id)}</Alert> }
                        .into_any()
                })
        }
    };
    let jobs = OnceResource::new(list_jobs());
    view! {
        <Layout title=t!("jobs.title")>
            {notice}
            <Suspense>
                {move || Suspend::new(async move {
                    match jobs.await {
                        Ok(jobs) => view! { <JobList jobs now=unix_now() /> }.into_any(),
                        Err(error) => failure_alert(&error).into_any(),
                    }
                })}
            </Suspense>
        </Layout>
    }
}
