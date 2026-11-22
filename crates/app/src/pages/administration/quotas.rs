//! `/administration/quotas`: the quota rules, grouped by whom they limit, with a form to create
//! one and, in each rule's `<details>`, forms to change, enable or disable, and delete it. Pool
//! rules also show what the group used, with Grant extra and Reset for the group.
//!
//! The server answers an accepted form with `/administration/quotas?done=<notice>` and a refused
//! one by rendering this page again with its [`QuotasProblem`] and what was typed.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::{Section, Sections, page_data};
use crate::{
    NotFound,
    components::ui::{
        Alert, Badge, ButtonVariant, Card, Empty, Field, INPUT, LABEL, Tone, button_class,
        small_button_class,
    },
    currency::Currency,
    layout::Layout,
    quotas::{Allowance, Metric, Period, on_printers},
    t, tf,
};

/// Whom a rule limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubjectChoice {
    Everyone,
    Group(i64),
    User(i64),
    Pool(i64),
}

impl SubjectChoice {
    /// The value the subject field posts: `everyone`, `group:<id>`, `user:<id>` or `pool:<id>`.
    pub fn value(self) -> String {
        match self {
            SubjectChoice::Everyone => "everyone".to_owned(),
            SubjectChoice::Group(id) => format!("group:{id}"),
            SubjectChoice::User(id) => format!("user:{id}"),
            SubjectChoice::Pool(id) => format!("pool:{id}"),
        }
    }

    /// The subject a posted value names, if it is well formed.
    pub fn parse(value: &str) -> Option<Self> {
        if value == "everyone" {
            return Some(SubjectChoice::Everyone);
        }
        let (kind, id) = value.split_once(':')?;
        let id = id.parse::<i64>().ok()?;
        match kind {
            "group" => Some(SubjectChoice::Group(id)),
            "user" => Some(SubjectChoice::User(id)),
            "pool" => Some(SubjectChoice::Pool(id)),
            _ => None,
        }
    }
}

/// A group or a user to choose from, by id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: i64,
    pub name: String,
}

/// A rule as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRow {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub subject: SubjectChoice,
    pub printers: Vec<String>,
    pub metric: Metric,
    pub period: Period,
    pub limit: i64,
    pub hard: bool,
    /// For a pool, what its group used in the current window; `None` for other rules, or when
    /// it could not be read.
    pub pool: Option<Allowance>,
}

/// The fields of a rule form, as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDraft {
    pub name: String,
    pub enabled: bool,
    pub subject: String,
    pub printers: Vec<String>,
    pub metric: String,
    pub period: String,
    pub rolling_days: String,
    pub limit: String,
    pub enforcement: String,
}

impl Default for RuleDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            subject: "everyone".to_owned(),
            printers: Vec::new(),
            metric: "pages".to_owned(),
            period: "month".to_owned(),
            rolling_days: String::new(),
            limit: String::new(),
            enforcement: "hard".to_owned(),
        }
    }
}

impl RuleDraft {
    /// The form of an existing rule.
    pub fn of(rule: &RuleRow, currency: &Currency) -> Self {
        Self {
            name: rule.name.clone(),
            enabled: rule.enabled,
            subject: rule.subject.value(),
            printers: rule.printers.clone(),
            metric: rule.metric.keyword().to_owned(),
            period: rule.period.keyword().to_owned(),
            rolling_days: match rule.period {
                Period::Rolling(days) => days.to_string(),
                _ => String::new(),
            },
            limit: rule.metric.field(rule.limit, currency),
            enforcement: if rule.hard { "hard" } else { "soft" }.to_owned(),
        }
    }
}

/// What the server hands the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotasPageData {
    /// `None` when they could not be loaded.
    pub rules: Option<Vec<RuleRow>>,
    pub groups: Vec<Choice>,
    pub users: Vec<Choice>,
    /// The printers CUPS lists and those rules name, to choose from.
    pub printers: Vec<String>,
    pub currency: Currency,
    pub refusal: Option<QuotasProblem>,
    /// What a refused form held, and the rule it was about (`None` for the New rule form).
    pub draft: Option<(Option<i64>, RuleDraft)>,
}

/// Why a form of the page was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotasProblem {
    NameEmpty,
    /// The limit is not a whole number, or not an amount in the currency for cost.
    LimitInvalid,
    LimitNegative,
    LimitTooLarge {
        maximum: i64,
    },
    /// The rule has grants or resets, and the form changes its metric or its kind of period.
    RuleAdjusted,
    RollingDaysInvalid {
        maximum: u16,
    },
    /// The group or user is gone, or the subject field names nothing.
    SubjectNotFound,
    /// A field other than those holds a value the page never offers.
    InvalidForm,
    RuleNotFound,
    ConfirmMissing,
    /// An extra allowance that is not more than 0, or not an amount.
    AmountInvalid,
    AmountTooLarge {
        maximum: i64,
    },
    /// A reason that is empty or longer than 200 characters.
    ReasonInvalid,
    /// The database failed.
    Unavailable,
}

impl QuotasProblem {
    pub(crate) fn message(self, currency: &Currency) -> String {
        match self {
            QuotasProblem::NameEmpty => t!("administration.quotas.errors.name_empty").to_owned(),
            QuotasProblem::LimitInvalid => tf!(
                "administration.quotas.errors.limit_invalid",
                currency = currency.code.clone()
            ),
            QuotasProblem::LimitNegative => {
                t!("administration.quotas.errors.limit_negative").to_owned()
            }
            QuotasProblem::LimitTooLarge { maximum } => tf!(
                "administration.quotas.errors.limit_too_large",
                maximum = maximum
            ),
            QuotasProblem::RuleAdjusted => {
                t!("administration.quotas.errors.rule_adjusted").to_owned()
            }
            QuotasProblem::RollingDaysInvalid { maximum } => {
                tf!(
                    "administration.quotas.errors.rolling_days",
                    maximum = maximum
                )
            }
            QuotasProblem::SubjectNotFound => {
                t!("administration.quotas.errors.subject_not_found").to_owned()
            }
            QuotasProblem::InvalidForm => {
                t!("administration.quotas.errors.invalid_form").to_owned()
            }
            QuotasProblem::RuleNotFound => t!("administration.quotas.errors.not_found").to_owned(),
            QuotasProblem::ConfirmMissing => {
                t!("administration.quotas.errors.confirm_missing").to_owned()
            }
            QuotasProblem::AmountInvalid => tf!(
                "administration.quotas.errors.amount_invalid",
                currency = currency.code.clone()
            ),
            QuotasProblem::AmountTooLarge { maximum } => tf!(
                "administration.quotas.errors.amount_too_large",
                maximum = maximum
            ),
            QuotasProblem::ReasonInvalid => {
                t!("administration.quotas.errors.reason_invalid").to_owned()
            }
            QuotasProblem::Unavailable => t!("administration.quotas.errors.unavailable").to_owned(),
        }
    }
}

/// What an accepted form did, named by the `done` query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotasNotice {
    Created,
    Saved,
    Enabled,
    Disabled,
    Deleted,
    Granted,
    Reset,
}

impl QuotasNotice {
    const ALL: [QuotasNotice; 7] = [
        QuotasNotice::Created,
        QuotasNotice::Saved,
        QuotasNotice::Enabled,
        QuotasNotice::Disabled,
        QuotasNotice::Deleted,
        QuotasNotice::Granted,
        QuotasNotice::Reset,
    ];

    /// The value of the `done` query parameter.
    pub fn code(self) -> &'static str {
        match self {
            QuotasNotice::Created => "created",
            QuotasNotice::Saved => "saved",
            QuotasNotice::Enabled => "enabled",
            QuotasNotice::Disabled => "disabled",
            QuotasNotice::Deleted => "deleted",
            QuotasNotice::Granted => "granted",
            QuotasNotice::Reset => "reset",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|notice| notice.code() == code)
    }

    fn message(self) -> &'static str {
        match self {
            QuotasNotice::Created => t!("administration.quotas.done.created"),
            QuotasNotice::Saved => t!("administration.quotas.done.saved"),
            QuotasNotice::Enabled => t!("administration.quotas.done.enabled"),
            QuotasNotice::Disabled => t!("administration.quotas.done.disabled"),
            QuotasNotice::Deleted => t!("administration.quotas.done.deleted"),
            QuotasNotice::Granted => t!("administration.quotas.done.granted"),
            QuotasNotice::Reset => t!("administration.quotas.done.reset"),
        }
    }
}

/// The names of groups and users, to word subjects.
struct Names<'a> {
    groups: &'a [Choice],
    users: &'a [Choice],
}

impl Names<'_> {
    fn of(list: &[Choice], id: i64) -> String {
        list.iter()
            .find(|choice| choice.id == id)
            .map_or_else(|| format!("#{id}"), |choice| choice.name.clone())
    }

    /// `Everyone`, `Each member of design`, `alice`, `design together`.
    fn subject(&self, subject: SubjectChoice) -> String {
        match subject {
            SubjectChoice::Everyone => t!("administration.quotas.subject.everyone").to_owned(),
            SubjectChoice::Group(id) => tf!(
                "administration.quotas.subject.group",
                group = Self::of(self.groups, id)
            ),
            SubjectChoice::User(id) => tf!(
                "administration.quotas.subject.user",
                user = Self::of(self.users, id)
            ),
            SubjectChoice::Pool(id) => tf!(
                "administration.quotas.subject.pool",
                group = Self::of(self.groups, id)
            ),
        }
    }
}

/// `50 color pages a month, on all printers, hard`.
fn summary(rule: &RuleRow, currency: &Currency) -> String {
    let limit = tf!(
        "administration.quotas.limit",
        amount = rule.metric.amount(rule.limit, currency),
        period = rule.period.each()
    );
    let printers = if rule.printers.is_empty() {
        tf!("administration.quotas.all_printers", text = limit)
    } else {
        on_printers(limit, &rule.printers)
    };
    let enforcement = if rule.hard {
        t!("administration.quotas.hard")
    } else {
        t!("administration.quotas.soft")
    };
    tf!(
        "administration.quotas.summary",
        text = printers,
        enforcement = enforcement
    )
}

#[component]
pub fn QuotasPage() -> impl IntoView {
    let Some((_, data)) = page_data::<QuotasPageData>() else {
        return view! { <NotFound /> }.into_any();
    };
    let message = data.refusal.map(|problem| problem.message(&data.currency));
    let alert = if let Some(message) = message {
        Some(view! { <Alert tone=Tone::Danger>{message}</Alert> }.into_any())
    } else {
        let query = use_query_map();
        let notice = query
            .read_untracked()
            .get("done")
            .and_then(|code| QuotasNotice::from_code(&code));
        notice
            .map(|notice| view! { <Alert tone=Tone::Success>{notice.message()}</Alert> }.into_any())
    };
    let (draft_rule, draft) = match data.draft.clone() {
        Some((rule, draft)) => (rule, Some(draft)),
        None => (None, None),
    };
    let new_draft = match (draft_rule, &draft) {
        (None, Some(draft)) => draft.clone(),
        _ => RuleDraft::default(),
    };
    let context = FormContext {
        groups: data.groups.clone(),
        users: data.users.clone(),
        printers: data.printers.clone(),
        currency: data.currency.clone(),
    };
    let list = match data.rules {
        None => view! {
            <Alert tone=Tone::Danger>{t!("administration.quotas.load_failed")}</Alert>
        }
        .into_any(),
        Some(rules) if rules.is_empty() => view! {
            <Empty title=t!("administration.quotas.empty") body=t!("administration.quotas.empty_body") />
        }
        .into_any(),
        Some(mut rules) => {
            rules.sort_by_key(|rule| (rule.subject, rule.id));
            let mut sections: Vec<(SubjectChoice, Vec<RuleRow>)> = Vec::new();
            for rule in rules {
                match sections.last_mut() {
                    Some((subject, list)) if *subject == rule.subject => list.push(rule),
                    _ => sections.push((rule.subject, vec![rule])),
                }
            }
            let names = Names {
                groups: &data.groups,
                users: &data.users,
            };
            sections
                .into_iter()
                .map(|(subject, rules)| {
                    let heading = names.subject(subject);
                    let rules = rules
                        .into_iter()
                        .map(|rule| {
                            let draft = (draft_rule == Some(rule.id))
                                .then(|| draft.clone())
                                .flatten();
                            view! { <Rule rule draft context=context.clone() /> }
                        })
                        .collect_view();
                    view! {
                        <section class="bg-card overflow-hidden rounded-lg border shadow-[0_1px_2px_rgba(0,0,0,0.08)]">
                            <h2 class="border-b px-4 py-3 text-base font-semibold md:px-6">{heading}</h2>
                            <ul class="divide-y">{rules}</ul>
                        </section>
                    }
                })
                .collect_view()
                .into_any()
        }
    };
    view! {
        <Layout title=t!("administration.title") document_title=t!("administration.quotas.document_title")>
            <Sections current=Section::Quotas />
            <p class="text-muted-foreground text-sm">{t!("administration.quotas.intro")}</p>
            {alert}
            <Card title=t!("administration.quotas.new")>
                <RuleForm
                    action="/administration/quotas".to_owned()
                    prefix="new".to_owned()
                    draft=new_draft
                    context=context.clone()
                    submit=t!("administration.quotas.create")
                />
            </Card>
            {list}
        </Layout>
    }
    .into_any()
}

/// What every rule form chooses from.
#[derive(Debug, Clone)]
struct FormContext {
    groups: Vec<Choice>,
    users: Vec<Choice>,
    printers: Vec<String>,
    currency: Currency,
}

#[component]
fn Rule(rule: RuleRow, draft: Option<RuleDraft>, context: FormContext) -> impl IntoView {
    let id = rule.id;
    let line = summary(&rule, &context.currency);
    let disabled = (!rule.enabled).then(|| {
        view! { <Badge tone=Tone::Warning>{t!("administration.quotas.disabled")}</Badge> }
    });
    let opened = draft.is_some();
    let draft = draft.unwrap_or_else(|| RuleDraft::of(&rule, &context.currency));
    let (toggle, toggle_label) = if rule.enabled {
        ("false", t!("administration.quotas.disable"))
    } else {
        ("true", t!("administration.quotas.enable"))
    };
    let pool = rule.pool.clone().map(|standing| {
        let used = standing.used(&context.currency);
        view! {
            <p class="text-sm">{used}</p>
            <Adjust
                action=format!("/administration/quotas/{id}")
                prefix=format!("pool-{id}")
                metric=rule.metric
                currency=context.currency.clone()
            />
        }
    });
    view! {
        <li class="grid gap-2 px-4 py-3 md:px-6">
            <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                <h3 class="font-semibold break-all">{rule.name.clone()}</h3>
                {disabled}
            </div>
            <p class="text-muted-foreground text-sm">{line}</p>
            {pool}
            <details class="text-sm" open=opened>
                <summary class="text-brand min-h-8 cursor-pointer font-semibold">
                    {tf!("administration.quotas.change", rule = rule.name.clone())}
                </summary>
                <div class="grid gap-4 pt-2">
                    <RuleForm
                        action=format!("/administration/quotas/{id}")
                        prefix=format!("rule-{id}")
                        draft
                        context=context.clone()
                        submit=t!("administration.quotas.save")
                    />
                    <div class="flex flex-wrap items-start gap-4">
                        <form method="post" action=format!("/administration/quotas/{id}/enabled")>
                            <input type="hidden" name="enabled" value=toggle />
                            <button type="submit" class=small_button_class(ButtonVariant::Secondary)>
                                {toggle_label}
                            </button>
                        </form>
                        <form
                            method="post"
                            action=format!("/administration/quotas/{id}/delete")
                            class="flex flex-wrap items-center gap-2"
                        >
                            <label class="flex min-h-8 items-center gap-2">
                                <input type="checkbox" name="confirm" value="yes" required class="accent-brand size-4" />
                                {t!("administration.quotas.confirm_delete")}
                            </label>
                            <button type="submit" class=small_button_class(ButtonVariant::Secondary)>
                                {t!("administration.quotas.delete")}
                            </button>
                        </form>
                    </div>
                </div>
            </details>
        </li>
    }
}

/// The fields of a rule. `prefix` keeps the ids of the page's forms apart.
#[component]
fn RuleForm(
    action: String,
    prefix: String,
    draft: RuleDraft,
    context: FormContext,
    submit: &'static str,
) -> impl IntoView {
    let id = |field: &str| format!("{prefix}-{field}");
    let (name_id, subject_id, metric_id, limit_id, period_id, days_id) = (
        id("name"),
        id("subject"),
        id("metric"),
        id("limit"),
        id("period"),
        id("rolling-days"),
    );
    let (limit_hint, days_hint) = (id("limit-hint"), id("rolling-days-hint"));
    let subject = draft.subject.clone();
    let option = move |value: String, label: String| {
        let selected = value == subject;
        view! { <option value=value selected=selected>{label}</option> }
    };
    let groups = context
        .groups
        .iter()
        .map(|group| {
            option(
                SubjectChoice::Group(group.id).value(),
                tf!(
                    "administration.quotas.subject.group",
                    group = group.name.clone()
                ),
            )
        })
        .collect_view();
    let pools = context
        .groups
        .iter()
        .map(|group| {
            option(
                SubjectChoice::Pool(group.id).value(),
                tf!(
                    "administration.quotas.subject.pool",
                    group = group.name.clone()
                ),
            )
        })
        .collect_view();
    let users = context
        .users
        .iter()
        .map(|user| {
            option(
                SubjectChoice::User(user.id).value(),
                tf!(
                    "administration.quotas.subject.user",
                    user = user.name.clone()
                ),
            )
        })
        .collect_view();
    let everyone = option(
        "everyone".to_owned(),
        t!("administration.quotas.subject.everyone").to_owned(),
    );
    let mut printers = context.printers.clone();
    for printer in &draft.printers {
        if !printers.contains(printer) {
            printers.push(printer.clone());
        }
    }
    let printers = printers
        .into_iter()
        .map(|printer| {
            let checked = draft.printers.contains(&printer);
            view! {
                <label class="flex min-h-8 items-center gap-2 text-sm">
                    <input
                        type="checkbox"
                        name="printers"
                        value=printer.clone()
                        checked=checked
                        class="accent-brand size-4"
                    />
                    {printer.clone()}
                </label>
            }
        })
        .collect_view();
    let metrics = Metric::ALL
        .into_iter()
        .map(|metric| {
            let selected = metric.keyword() == draft.metric;
            view! { <option value=metric.keyword() selected=selected>{metric.label()}</option> }
        })
        .collect_view();
    let periods = Period::KEYWORDS
        .into_iter()
        .map(|keyword| {
            let selected = keyword == draft.period;
            view! { <option value=keyword selected=selected>{Period::label(keyword)}</option> }
        })
        .collect_view();
    let hard = draft.enforcement != "soft";
    view! {
        <form method="post" action=action class="grid gap-4 sm:grid-cols-2">
            <Field label=t!("administration.quotas.fields.name") input_id=name_id.clone()>
                <input
                    id=name_id
                    name="name"
                    type="text"
                    value=draft.name
                    required
                    maxlength="100"
                    autocomplete="off"
                    class=INPUT
                />
            </Field>
            <Field label=t!("administration.quotas.fields.subject") input_id=subject_id.clone()>
                <select id=subject_id name="subject" class=INPUT>
                    {everyone}
                    <optgroup label=t!("administration.quotas.fields.groups")>{groups}</optgroup>
                    <optgroup label=t!("administration.quotas.fields.pools")>{pools}</optgroup>
                    <optgroup label=t!("administration.quotas.fields.users")>{users}</optgroup>
                </select>
            </Field>
            <Field label=t!("administration.quotas.fields.metric") input_id=metric_id.clone()>
                <select id=metric_id name="metric" class=INPUT>
                    {metrics}
                </select>
            </Field>
            <Field
                label=t!("administration.quotas.fields.limit")
                input_id=limit_id.clone()
                hint=tf!("administration.quotas.fields.limit_hint", currency = context.currency.code.clone())
            >
                <input
                    id=limit_id
                    name="limit"
                    type="text"
                    inputmode="decimal"
                    value=draft.limit
                    required
                    autocomplete="off"
                    aria-describedby=limit_hint
                    class=INPUT
                />
            </Field>
            <Field label=t!("administration.quotas.fields.period") input_id=period_id.clone()>
                <select id=period_id name="period" class=INPUT>
                    {periods}
                </select>
            </Field>
            <Field
                label=t!("administration.quotas.fields.rolling_days")
                input_id=days_id.clone()
                hint=t!("administration.quotas.fields.rolling_days_hint")
            >
                <input
                    id=days_id
                    name="rolling_days"
                    type="number"
                    min="1"
                    max="366"
                    value=draft.rolling_days
                    aria-describedby=days_hint
                    class=INPUT
                />
            </Field>
            <fieldset class="grid content-start gap-1.5">
                <legend class=LABEL>{t!("administration.quotas.fields.printers")}</legend>
                <p class="text-muted-foreground text-sm">{t!("administration.quotas.fields.printers_hint")}</p>
                {printers}
            </fieldset>
            <fieldset class="grid content-start gap-1.5">
                <legend class=LABEL>{t!("administration.quotas.fields.enforcement")}</legend>
                <label class="flex min-h-8 items-center gap-2 text-sm">
                    <input type="radio" name="enforcement" value="hard" checked=hard class="accent-brand size-4" />
                    {t!("administration.quotas.fields.hard")}
                </label>
                <label class="flex min-h-8 items-center gap-2 text-sm">
                    <input type="radio" name="enforcement" value="soft" checked=!hard class="accent-brand size-4" />
                    {t!("administration.quotas.fields.soft")}
                </label>
            </fieldset>
            <label class=format!("{LABEL} flex min-h-10 items-center gap-2 sm:col-span-2")>
                <input type="checkbox" name="enabled" value="true" checked=draft.enabled class="accent-brand size-4" />
                {t!("administration.quotas.fields.enabled")}
            </label>
            <div class="sm:col-span-2">
                <button type="submit" class=button_class(ButtonVariant::Primary)>
                    {submit}
                </button>
            </div>
        </form>
    }
}

/// Grant extra and Reset, posted to `{action}/grant` and `{action}/reset`. The Users page uses it
/// too, with the rule in a hidden field.
#[component]
pub(crate) fn Adjust(
    action: String,
    prefix: String,
    metric: Metric,
    currency: Currency,
    #[prop(optional)] rule: Option<i64>,
) -> impl IntoView {
    let id = |field: &str| format!("{prefix}-{field}");
    let (amount_id, grant_reason_id, reset_reason_id) =
        (id("amount"), id("grant-reason"), id("reset-reason"));
    let (amount_hint, reset_hint) = (id("amount-hint"), id("reset-reason-hint"));
    let rule_field = move || {
        rule.map(|rule| view! { <input type="hidden" name="rule" value=rule.to_string() /> })
    };
    let hint = match metric {
        Metric::Cost => tf!(
            "administration.quotas.adjust.amount_cost",
            currency = currency.code.clone()
        ),
        _ => t!("administration.quotas.adjust.amount_count").to_owned(),
    };
    view! {
        <details class="text-sm">
            <summary class="text-brand min-h-8 cursor-pointer font-semibold">
                {t!("administration.quotas.adjust.summary")}
            </summary>
            <div class="grid gap-4 pt-2 sm:grid-cols-2">
                <form method="post" action=format!("{action}/grant") class="grid content-start gap-2">
                    {rule_field()}
                    <Field label=t!("administration.quotas.adjust.amount") input_id=amount_id.clone() hint=hint>
                        <input
                            id=amount_id
                            name="amount"
                            type="text"
                            inputmode="decimal"
                            required
                            autocomplete="off"
                            aria-describedby=amount_hint
                            class=INPUT
                        />
                    </Field>
                    <Field label=t!("administration.quotas.adjust.reason") input_id=grant_reason_id.clone()>
                        <input
                            id=grant_reason_id
                            name="reason"
                            type="text"
                            required
                            maxlength="200"
                            autocomplete="off"
                            class=INPUT
                        />
                    </Field>
                    <div>
                        <button type="submit" class=small_button_class(ButtonVariant::Primary)>
                            {t!("administration.quotas.adjust.grant")}
                        </button>
                    </div>
                </form>
                <form method="post" action=format!("{action}/reset") class="grid content-start gap-2">
                    {rule_field()}
                    <Field
                        label=t!("administration.quotas.adjust.reason")
                        input_id=reset_reason_id.clone()
                        hint=t!("administration.quotas.adjust.reset_hint")
                    >
                        <input
                            id=reset_reason_id
                            name="reason"
                            type="text"
                            required
                            maxlength="200"
                            autocomplete="off"
                            aria-describedby=reset_hint
                            class=INPUT
                        />
                    </Field>
                    <div>
                        <button type="submit" class=small_button_class(ButtonVariant::Secondary)>
                            {t!("administration.quotas.adjust.reset")}
                        </button>
                    </div>
                </form>
            </div>
        </details>
    }
}
