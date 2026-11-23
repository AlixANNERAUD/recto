//! `/jobs`: the documents waiting for the user, sent from their computers and phones, then
//! print jobs, with Cancel for active ones. The print form and a release land here with
//! `?submitted=<id>`, and `&over=<rule id>,<rule id>` when the job went over soft quota rules;
//! a refused cancel or release with `?error=<code>` (`&rule=<id>` for a quota), and a deleted
//! document with `?done=deleted`.
//!
//! Each waiting document has its own plain forms: release, with the options its device asked
//! for, and delete. They post to `/jobs/held/<id>/release` and `/jobs/held/<id>/delete`.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use recto_printing::Printer;

use super::{failure_alert, print::quota_refusal};
use crate::{
    components::ui::{
        Alert, Badge, ButtonVariant, Card, Field, INPUT, Tone, button_class, small_button_class,
    },
    currency::Currency,
    display::{iso_time, relative_time, unix_now},
    functions::{Allowances, HeldRow, list_jobs, list_printers, my_held_jobs, my_quotas},
    islands::jobs::JobList,
    layout::Layout,
    refusal::Refusal,
    t, tf, tp,
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
            let rule = query.get("rule").and_then(|id| id.parse::<i64>().ok());
            (Some(refusal_alert(refusal, rule)), Vec::new())
        } else if query.get("done").is_some_and(|done| done == "deleted") {
            (
                Some(
                    view! { <Alert tone=Tone::Success>{t!("jobs.held.deleted")}</Alert> }
                        .into_any(),
                ),
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
    let held = OnceResource::new(my_held_jobs());
    let printers = OnceResource::new(list_printers());
    let currency = currency();
    view! {
        <Layout title=t!("jobs.title")>
            {notice}
            {warnings}
            <Suspense>
                {move || Suspend::new(async move {
                    match held.await {
                        // Absent until a device sends something.
                        Ok(rows) if rows.is_empty() => None,
                        Ok(rows) => {
                            let printers = printers.await;
                            Some(view! { <Waiting rows printers now=unix_now() /> }.into_any())
                        }
                        Err(error) => Some(failure_alert(&error).into_any()),
                    }
                })}
            </Suspense>
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

/// A refused form's message; a quota refusal names the rule and what is left of it, read now,
/// as on the Print page.
fn refusal_alert(refusal: Refusal, rule: Option<i64>) -> AnyView {
    let Some(rule) = rule.filter(|_| refusal == Refusal::Quota) else {
        return view! { <Alert tone=Tone::Danger>{refusal.message()}</Alert> }.into_any();
    };
    let quotas = OnceResource::new(my_quotas());
    view! {
        <Suspense>
            {move || Suspend::new(async move {
                let message = quotas
                    .await
                    .ok()
                    .and_then(|quotas| quota_refusal(&quotas, rule))
                    .unwrap_or_else(|| refusal.message());
                view! { <Alert tone=Tone::Danger>{message}</Alert> }
            })}
        </Suspense>
    }
    .into_any()
}

/// The documents waiting for the user, each with its release and delete forms. Without the
/// printers, only Delete is offered.
#[component]
fn Waiting(
    rows: Vec<HeldRow>,
    printers: Result<Vec<Printer>, ServerFnError>,
    now: i64,
) -> impl IntoView {
    let (printers, problem) = match printers {
        Ok(printers) => (printers, None),
        Err(error) => (Vec::new(), Some(failure_alert(&error))),
    };
    // The default printer is the one a release starts from.
    let default = printers
        .iter()
        .find(|printer| printer.is_default)
        .or(printers.first())
        .map(|printer| printer.name.clone());
    let names: Vec<String> = printers.into_iter().map(|printer| printer.name).collect();
    let no_printers = (problem.is_none() && names.is_empty())
        .then(|| view! { <p class="text-sm">{t!("jobs.held.no_printers")}</p> });
    view! {
        <Card title=t!("jobs.held.title")>
            <div class="grid gap-4">
                <p class="text-muted-foreground text-sm">{t!("jobs.held.intro")}</p>
                {problem}
                {no_printers}
                <ul class="divide-y rounded-md border">
                    {rows
                        .into_iter()
                        .map(|row| {
                            view! {
                                <HeldItem
                                    row
                                    printers=names.clone()
                                    default=default.clone()
                                    now
                                />
                            }
                        })
                        .collect_view()}
                </ul>
            </div>
        </Card>
    }
}

#[component]
fn HeldItem(
    row: HeldRow,
    printers: Vec<String>,
    default: Option<String>,
    now: i64,
) -> impl IntoView {
    let id = row.id;
    let name = row.name.clone();
    let details = [
        row.device
            .clone()
            .map(|device| tf!("jobs.held.from", device = device)),
        row.pages.map(|pages| tp!("jobs.pages", pages)),
    ]
    .into_iter()
    .flatten()
    .map(|detail| {
        view! {
            <span aria-hidden="true">"·"</span>
            <span>{detail}</span>
        }
    })
    .collect_view();
    let refusal = row
        .refusal
        .as_deref()
        .and_then(Refusal::from_code)
        .map(|refusal| {
            view! {
                <Alert tone=Tone::Warning>
                    {tf!("jobs.held.last_refusal", reason = refusal.message())}
                </Alert>
            }
        });
    let actions = if row.releasing {
        view! { <Badge tone=Tone::Brand>{t!("jobs.held.releasing")}</Badge> }.into_any()
    } else {
        let release = (!printers.is_empty())
            .then(|| view! { <ReleaseForm row=row.clone() printers default /> });
        view! {
            {release}
            <form method="post" action=format!("/jobs/held/{id}/delete")>
                <button
                    type="submit"
                    class=small_button_class(ButtonVariant::Secondary)
                    aria-label=tf!("jobs.held.delete_label", name = name.clone())
                >
                    {t!("jobs.held.delete")}
                </button>
            </form>
        }
        .into_any()
    };
    view! {
        <li class="grid gap-3 px-4 py-3">
            <div class="flex flex-wrap items-start justify-between gap-2">
                <div class="min-w-0 flex-1 basis-56">
                    <p class="font-medium break-words">{row.name.clone()}</p>
                    <p class="text-muted-foreground flex flex-wrap gap-x-2 text-sm">
                        <time datetime=iso_time(row.received_at)>
                            {tf!(
                                "jobs.held.received",
                                time = relative_time(row.received_at, now)
                            )}
                        </time>
                        {details}
                    </p>
                </div>
                <a
                    href=format!("/jobs/held/{id}/document")
                    target="_blank"
                    class=small_button_class(ButtonVariant::Secondary)
                    aria-label=tf!("jobs.held.preview_label", name = name.clone())
                >
                    {t!("jobs.held.preview")}
                </a>
            </div>
            {refusal}
            {actions}
        </li>
    }
}

/// The release form, filled with what the device asked for. Every option is offered: the
/// printer's own limits are checked when it is sent, as on the Print page without JavaScript.
#[component]
fn ReleaseForm(row: HeldRow, printers: Vec<String>, default: Option<String>) -> impl IntoView {
    let id = row.id;
    let field = move |name: &str| format!("held-{id}-{name}");
    let sides = [
        ("one-sided", t!("print.sides.one_sided")),
        ("two-sided-long-edge", t!("print.sides.long_edge")),
        ("two-sided-short-edge", t!("print.sides.short_edge")),
    ];
    let colors = [
        ("color", t!("print.color.color")),
        ("monochrome", t!("print.color.monochrome")),
    ];
    let chosen_sides = row.sides.clone();
    // On wide screens, columns sized for their longest choice: "Two-sided, flip on short edge"
    // and "Black and white" are cut in equal columns.
    let chosen_color = row.color_mode.clone();
    view! {
        <form
            method="post"
            action=format!("/jobs/held/{id}/release")
            class="grid gap-3 sm:grid-cols-2 lg:grid-cols-[1fr_5rem_2fr_1.4fr_1fr]"
        >
            <Field label=t!("print.printer") input_id=field("printer")>
                <select id=field("printer") name="printer" required class=INPUT>
                    {printers
                        .into_iter()
                        .map(|printer| {
                            let selected = default.as_deref() == Some(printer.as_str());
                            view! {
                                <option value=printer.clone() selected=selected>
                                    {printer.clone()}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </Field>
            <Field label=t!("print.copies") input_id=field("copies")>
                <input
                    id=field("copies")
                    name="copies"
                    type="number"
                    min="1"
                    max="99"
                    value=row.copies.unwrap_or(1).to_string()
                    class=INPUT
                />
            </Field>
            <Field label=t!("print.sides.label") input_id=field("sides")>
                <select id=field("sides") name="sides" class=INPUT>
                    {sides
                        .into_iter()
                        .map(|(value, label)| {
                            let selected = chosen_sides.as_deref() == Some(value);
                            view! {
                                <option value=value selected=selected>
                                    {label}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </Field>
            <Field label=t!("print.color.label") input_id=field("color")>
                <select id=field("color") name="color_mode" class=INPUT>
                    // What the server does without a choice: color on a color printer.
                    <option value="">{t!("jobs.held.color_when_possible")}</option>
                    {colors
                        .into_iter()
                        .map(|(value, label)| {
                            let selected = chosen_color.as_deref() == Some(value);
                            view! {
                                <option value=value selected=selected>
                                    {label}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </Field>
            <Field
                label=t!("print.pages.label")
                input_id=field("pages")
                hint=t!("print.pages.hint")
            >
                <input
                    id=field("pages")
                    name="page_ranges"
                    type="text"
                    placeholder=t!("print.pages.placeholder")
                    value=row.page_ranges.clone().unwrap_or_default()
                    aria-describedby=format!("held-{id}-pages-hint")
                    class=INPUT
                />
            </Field>
            <div class="sm:col-span-2 lg:col-span-5">
                <button
                    type="submit"
                    class=button_class(ButtonVariant::Primary)
                    aria-label=tf!("jobs.held.release_label", name = row.name.clone())
                >
                    {t!("jobs.held.release")}
                </button>
            </div>
        </form>
    }
}
