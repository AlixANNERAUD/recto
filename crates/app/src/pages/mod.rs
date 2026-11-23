//! The pages of the interface.

pub mod account;
pub mod administration;
pub mod auth;
pub mod jobs;
pub mod no_access;
pub mod offline;
pub mod print;
pub mod printers;
pub mod scan;
pub mod sign_in;

use leptos::prelude::*;

use crate::{
    components::{
        error_text,
        ui::{Alert, Tone},
    },
    functions::Allowances,
    refusal::Refusal,
    tf,
};

/// The message of a page whose data could not be loaded, such as CUPS being unreachable: the
/// page still renders around it.
pub(crate) fn failure_alert(error: &ServerFnError) -> impl IntoView + use<> {
    let message = error_text(error);
    view! { <Alert tone=Tone::Danger>{message}</Alert> }
}

/// A refused form's message. A quota refusal names the rule and what is left of it, read from
/// `quotas` now, since only the rule's id travels in the address; `allowances` finds them in
/// what the resource loads. Other refusals need nothing loaded.
pub(crate) fn refusal_alert<T: Clone + Send + Sync + 'static, Ser: 'static>(
    refusal: Refusal,
    rule: Option<i64>,
    quotas: impl FnOnce() -> OnceResource<T, Ser>,
    allowances: fn(T) -> Option<Allowances>,
) -> AnyView {
    let Some(rule) = rule.filter(|_| refusal == Refusal::Quota) else {
        return view! { <Alert tone=Tone::Danger>{refusal.message()}</Alert> }.into_any();
    };
    let quotas = quotas();
    view! {
        <Suspense>
            {move || Suspend::new(async move {
                let message = allowances(quotas.await)
                    .and_then(|quotas| quota_refusal(&quotas, rule))
                    .unwrap_or_else(|| refusal.message());
                view! { <Alert tone=Tone::Danger>{message}</Alert> }
            })}
        </Suspense>
    }
    .into_any()
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
