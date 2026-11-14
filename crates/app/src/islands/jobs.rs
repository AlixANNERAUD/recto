//! The job list. Rendered on the server it is complete, with a plain Cancel form per active
//! job; once the island runs it reloads the list every five seconds.

use std::time::Duration;

use leptos::{prelude::*, task::spawn_local};
use recto_printing::{Job, JobState};

use super::browser;
use crate::{
    components::ui::{Badge, ButtonVariant, Empty, Tone, button_class},
    display::{iso_time, relative_time, unix_now},
    functions::list_jobs,
    t, tf, tp,
};

const REFRESH: Duration = Duration::from_secs(5);

fn state_label(state: JobState) -> (&'static str, Tone) {
    match state {
        JobState::Pending => (t!("jobs.state.pending"), Tone::Neutral),
        JobState::Held => (t!("jobs.state.held"), Tone::Warning),
        JobState::Processing => (t!("jobs.state.processing"), Tone::Brand),
        JobState::Stopped => (t!("jobs.state.stopped"), Tone::Danger),
        JobState::Canceled => (t!("jobs.state.canceled"), Tone::Neutral),
        JobState::Aborted => (t!("jobs.state.aborted"), Tone::Danger),
        JobState::Completed => (t!("jobs.state.completed"), Tone::Success),
    }
}

#[component]
fn JobRow(job: Job, now: RwSignal<i64>) -> impl IntoView {
    let (state, tone) = state_label(job.state);
    let name = job
        .name
        .clone()
        .unwrap_or_else(|| t!("jobs.untitled").to_string());
    // CUPS hides other users' jobs unless told otherwise; the job is still there.
    let owner = job
        .owner
        .clone()
        .unwrap_or_else(|| t!("jobs.another_user").to_string());
    let pages = job.pages.map(|pages| tp!("jobs.pages", pages));
    let time = job.created.map(|created| {
        view! {
            <time datetime=iso_time(created)>{move || relative_time(created, now.get())}</time>
        }
    });
    let cancel = job.is_active().then(|| {
        view! {
            <form method="post" action=format!("/jobs/{}/cancel", job.id)>
                <button
                    type="submit"
                    class=button_class(ButtonVariant::Secondary)
                    aria-label=tf!("jobs.cancel_label", name = name.clone())
                >
                    {t!("jobs.cancel")}
                </button>
            </form>
        }
    });
    view! {
        <li class="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
            <div class="min-w-0 flex-1 basis-56">
                <p class="truncate font-medium">{name}</p>
                <p class="text-muted-foreground flex flex-wrap gap-x-2 text-sm">
                    <span>{tf!("jobs.number", id = job.id)}</span>
                    <span aria-hidden="true">"·"</span>
                    <span>{job.printer.clone()}</span>
                    <span aria-hidden="true">"·"</span>
                    <span>{owner}</span>
                    {pages
                        .map(|pages| {
                            view! {
                                <span aria-hidden="true">"·"</span>
                                <span>{pages}</span>
                            }
                        })}
                    {time
                        .map(|time| {
                            view! {
                                <span aria-hidden="true">"·"</span>
                                {time}
                            }
                        })}
                </p>
            </div>
            <Badge tone>{state}</Badge>
            {cancel}
        </li>
    }
    .into_any()
}

/// The jobs, newest first. `now` is the server's clock when it rendered, so the times written
/// on the server and in the browser agree until the first refresh.
#[island]
pub fn JobList(jobs: Vec<Job>, now: i64) -> impl IntoView {
    let jobs = RwSignal::new(jobs);
    let now = RwSignal::new(now);
    Effect::new(move |_| {
        browser::every(REFRESH, move || {
            spawn_local(async move {
                // A failed refresh keeps the list as it was; the next one may work.
                if let Ok(fresh) = list_jobs().await
                    && jobs.with_untracked(|current| *current != fresh)
                {
                    jobs.set(fresh);
                }
                now.set(unix_now());
            });
        });
    });
    view! {
        <Show
            when=move || jobs.with(|jobs| !jobs.is_empty())
            fallback=|| view! { <Empty title=t!("jobs.empty") body=t!("jobs.empty_body") /> }
        >
            <ul aria-label=t!("jobs.list") class="bg-card divide-y rounded-lg border shadow-[0_1px_2px_rgba(0,0,0,0.08)]">
                // Keyed by what a row shows, so unchanged rows keep their elements (and the
                // keyboard focus on their Cancel button) across refreshes.
                <For
                    each=move || jobs.get()
                    key=|job| (job.id, job.state as u8, job.pages, job.owner.clone(), job.name.clone())
                    children=move |job| view! { <JobRow job now /> }
                />
            </ul>
        </Show>
    }
    .into_any()
}
