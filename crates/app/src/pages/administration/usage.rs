//! `/administration/usage`: what was printed and scanned over a range of days, in total and by
//! user, group, printer or scanner. The range is a plain GET form and links, so the page works
//! without JavaScript; the server answers a range it cannot read with 400 and this page without
//! tables.

use leptos::prelude::*;

use super::{Section, Sections, page_data};
use crate::{
    NotFound,
    components::ui::{
        Alert, ButtonVariant, Card, Empty, Field, INPUT, Tone, button_class, small_button_class,
    },
    currency::Currency,
    layout::Layout,
    quotas::Metric,
    t,
};

/// Where the page is.
pub const USAGE: &str = "/administration/usage";

/// What the table's rows are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Breakdown {
    #[default]
    User,
    Group,
    Printer,
    Scanner,
}

impl Breakdown {
    pub const ALL: [Breakdown; 4] = [
        Breakdown::User,
        Breakdown::Group,
        Breakdown::Printer,
        Breakdown::Scanner,
    ];

    /// The value of the `by` query parameter.
    pub fn keyword(self) -> &'static str {
        match self {
            Breakdown::User => "user",
            Breakdown::Group => "group",
            Breakdown::Printer => "printer",
            Breakdown::Scanner => "scanner",
        }
    }

    /// The table a `by` value names; anything else is the user table.
    pub fn from_keyword(keyword: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|by| by.keyword() == keyword)
            .unwrap_or_default()
    }

    fn label(self) -> &'static str {
        match self {
            Breakdown::User => t!("administration.usage.by.user"),
            Breakdown::Group => t!("administration.usage.by.group"),
            Breakdown::Printer => t!("administration.usage.by.printer"),
            Breakdown::Scanner => t!("administration.usage.by.scanner"),
        }
    }

    /// The heading of the name column.
    fn column(self) -> &'static str {
        match self {
            Breakdown::User => t!("administration.usage.columns.user"),
            Breakdown::Group => t!("administration.usage.columns.group"),
            Breakdown::Printer => t!("administration.usage.columns.printer"),
            Breakdown::Scanner => t!("administration.usage.columns.scanner"),
        }
    }

    fn prints(self) -> bool {
        self != Breakdown::Scanner
    }

    fn scans(self) -> bool {
        self != Breakdown::Printer
    }
}

/// What was printed, from the ledger, refunded jobs left out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrintTotals {
    pub jobs: i64,
    pub pages: i64,
    pub sheets: i64,
    pub color_pages: i64,
    pub mono_pages: i64,
    /// In minor currency units.
    pub cost: i64,
    /// Jobs whose page counts are unknown.
    pub uncounted: i64,
}

/// What was scanned, from the scan log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanTotals {
    pub scans: i64,
    pub pages: i64,
    pub saved: i64,
}

/// A row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRow {
    pub label: String,
    /// The username under a display name.
    pub detail: Option<String>,
    pub print: Option<PrintTotals>,
    pub scan: Option<ScanTotals>,
}

/// The totals of the range and the rows of the chosen table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub print: PrintTotals,
    pub scan: ScanTotals,
    pub rows: Vec<UsageRow>,
}

/// What the server hands the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsagePageData {
    /// The dates of the range as `YYYY-MM-DD`, or as typed when they were refused.
    pub from: String,
    pub to: String,
    pub this_month: (String, String),
    pub last_month: (String, String),
    pub by: Breakdown,
    pub currency: Currency,
    /// `None` when it could not be loaded, or the dates were refused.
    pub report: Option<Report>,
    pub invalid_dates: bool,
}

/// The page's address for a range and a table. The dates are recto's own `YYYY-MM-DD`, safe in
/// a query.
pub fn usage_href(from: &str, to: &str, by: Breakdown) -> String {
    format!("{USAGE}?from={from}&to={to}&by={}", by.keyword())
}

/// The address of an export (`print-jobs.csv`, `scans.csv` or `totals.csv`) for a range and a
/// table.
pub fn export_href(file: &str, from: &str, to: &str, by: Breakdown) -> String {
    format!("{USAGE}/{file}?from={from}&to={to}&by={}", by.keyword())
}

/// What the anonymous network scans are called in the user table.
pub fn anonymous_label() -> &'static str {
    t!("administration.usage.anonymous")
}

const LINK: &str = "inline-flex min-h-10 items-center rounded-md border px-3 text-sm font-semibold transition-colors";

fn link_class(selected: bool) -> String {
    let colors = if selected {
        "border-brand bg-brand-subtle text-brand"
    } else {
        "border-input bg-card text-foreground hover:bg-muted"
    };
    format!("{LINK} {colors}")
}

#[component]
pub fn UsagePage() -> impl IntoView {
    let Some((_, data)) = page_data::<UsagePageData>() else {
        return view! { <NotFound /> }.into_any();
    };
    let by = data.by;
    let (this_from, this_to) = data.this_month.clone();
    let (last_from, last_to) = data.last_month.clone();
    let is_this = data.from == this_from && data.to == this_to;
    let is_last = data.from == last_from && data.to == last_to;
    let content = if data.invalid_dates {
        view! { <Alert tone=Tone::Danger>{t!("administration.usage.invalid_dates")}</Alert> }
            .into_any()
    } else {
        match data.report.clone() {
            None => view! {
                <Alert tone=Tone::Danger>{t!("administration.usage.load_failed")}</Alert>
            }
            .into_any(),
            Some(report) => view! {
                <div class="flex flex-wrap items-center justify-between gap-3">
                    <Tables from=data.from.clone() to=data.to.clone() by />
                    <Downloads from=data.from.clone() to=data.to.clone() by />
                </div>
                <Total report=report.clone() currency=data.currency.clone() />
                <UsageTable rows=report.rows by currency=data.currency.clone() />
            }
            .into_any(),
        }
    };
    view! {
        <Layout title=t!("administration.title") document_title=t!("administration.usage.document_title")>
            <Sections current=Section::Usage />
            <p class="text-muted-foreground text-sm">{t!("administration.usage.intro")}</p>
            <Card title=t!("administration.usage.range")>
                <div class="grid gap-4">
                    <ul class="flex flex-wrap gap-2">
                        <li>
                            <a
                                href=usage_href(&this_from, &this_to, by)
                                aria-current=is_this.then_some("true")
                                class=link_class(is_this)
                            >
                                {t!("administration.usage.this_month")}
                            </a>
                        </li>
                        <li>
                            <a
                                href=usage_href(&last_from, &last_to, by)
                                aria-current=is_last.then_some("true")
                                class=link_class(is_last)
                            >
                                {t!("administration.usage.last_month")}
                            </a>
                        </li>
                    </ul>
                    <form method="get" action=USAGE class="flex flex-wrap items-end gap-3">
                        <Field label=t!("administration.usage.from") input_id="usage-from">
                            <input type="date" id="usage-from" name="from" value=data.from.clone() required class=INPUT />
                        </Field>
                        <Field label=t!("administration.usage.to") input_id="usage-to">
                            <input type="date" id="usage-to" name="to" value=data.to.clone() required class=INPUT />
                        </Field>
                        <input type="hidden" name="by" value=by.keyword() />
                        <button type="submit" class=button_class(ButtonVariant::Secondary)>
                            {t!("administration.usage.show")}
                        </button>
                    </form>
                </div>
            </Card>
            {content}
        </Layout>
    }
    .into_any()
}

/// Links to the four tables, keeping the range.
#[component]
fn Tables(from: String, to: String, by: Breakdown) -> impl IntoView {
    let links = Breakdown::ALL
        .into_iter()
        .map(|table| {
            let selected = table == by;
            view! {
                <li>
                    <a
                        href=usage_href(&from, &to, table)
                        aria-current=selected.then_some("page")
                        class=link_class(selected)
                    >
                        {table.label()}
                    </a>
                </li>
            }
        })
        .collect_view();
    view! {
        <nav aria-label=t!("administration.usage.tables")>
            <ul class="flex flex-wrap gap-2">{links}</ul>
        </nav>
    }
}

/// Links to the CSV files of the range: the ledger, the scan log and the table shown.
#[component]
fn Downloads(from: String, to: String, by: Breakdown) -> impl IntoView {
    let links = [
        ("print-jobs.csv", t!("administration.usage.download.print_jobs")),
        ("scans.csv", t!("administration.usage.download.scans")),
        ("totals.csv", t!("administration.usage.download.totals")),
    ]
    .into_iter()
    .map(|(file, label)| {
        view! {
            <li>
                <a href=export_href(file, &from, &to, by) class=small_button_class(ButtonVariant::Subtle)>
                    {label}
                </a>
            </li>
        }
    })
    .collect_view();
    view! {
        <div class="flex flex-wrap items-center gap-2">
            <span id="usage-downloads" class="text-muted-foreground text-sm font-semibold">
                {t!("administration.usage.download.title")}
            </span>
            <ul aria-labelledby="usage-downloads" class="flex flex-wrap gap-1">{links}</ul>
        </div>
    }
}

/// The totals of the whole range.
#[component]
fn Total(report: Report, currency: Currency) -> impl IntoView {
    let tiles = [
        (Metric::Pages.label(), report.print.pages.to_string()),
        (Metric::Jobs.label(), report.print.jobs.to_string()),
        (Metric::Cost.label(), currency.format(report.print.cost)),
        (
            t!("administration.usage.columns.scanned_pages"),
            report.scan.pages.to_string(),
        ),
        (
            t!("administration.usage.columns.scans"),
            report.scan.scans.to_string(),
        ),
    ];
    let tiles = tiles
        .into_iter()
        .map(|(label, value)| {
            view! {
                <div class="grid gap-0.5">
                    <dt class="text-muted-foreground text-xs font-semibold">{label}</dt>
                    <dd class="text-xl font-semibold tabular-nums">{value}</dd>
                </div>
            }
        })
        .collect_view();
    view! {
        <Card title=t!("administration.usage.total")>
            <dl class="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-5">{tiles}</dl>
        </Card>
    }
}

/// Headings wrap, so that the numbers stay in view on a laptop without scrolling sideways.
const HEAD: &str = "text-muted-foreground px-2 py-2 align-bottom text-xs font-semibold";
const NUMBER: &str = "px-2 py-3 text-right tabular-nums whitespace-nowrap";

/// The chosen table, which scrolls sideways inside its card when the window is narrow.
#[component]
fn UsageTable(rows: Vec<UsageRow>, by: Breakdown, currency: Currency) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <Empty title=t!("administration.usage.empty") body=t!("administration.usage.empty_body") />
        }
        .into_any();
    }
    let mut headings = Vec::new();
    if by.prints() {
        headings.extend([
            Metric::Jobs.label(),
            Metric::Pages.label(),
            Metric::Sheets.label(),
            Metric::ColorPages.label(),
            Metric::MonoPages.label(),
            Metric::Cost.label(),
            t!("administration.usage.columns.uncounted"),
        ]);
    }
    if by.scans() {
        headings.extend([
            t!("administration.usage.columns.scans"),
            t!("administration.usage.columns.scanned_pages"),
            t!("administration.usage.columns.saved"),
        ]);
    }
    let headings = headings
        .into_iter()
        .map(|heading| view! { <th scope="col" class=format!("{HEAD} text-right")>{heading}</th> })
        .collect_view();
    let rows = rows
        .into_iter()
        .map(|row| {
            let mut cells = Vec::new();
            if by.prints() {
                let print = row.print.unwrap_or_default();
                cells.extend([
                    print.jobs.to_string(),
                    print.pages.to_string(),
                    print.sheets.to_string(),
                    print.color_pages.to_string(),
                    print.mono_pages.to_string(),
                    currency.format(print.cost),
                    print.uncounted.to_string(),
                ]);
            }
            if by.scans() {
                let scan = row.scan.unwrap_or_default();
                cells.extend([
                    scan.scans.to_string(),
                    scan.pages.to_string(),
                    scan.saved.to_string(),
                ]);
            }
            let cells = cells
                .into_iter()
                .map(|cell| view! { <td class=NUMBER>{cell}</td> })
                .collect_view();
            let detail = row
                .detail
                .map(|detail| view! { <span class="text-muted-foreground block">{detail}</span> });
            view! {
                <tr>
                    <th scope="row" class="bg-card sticky left-0 px-2 py-3 pl-4 text-left font-normal md:pl-6">
                        <span class="block font-semibold whitespace-nowrap">{row.label}</span>
                        {detail}
                    </th>
                    {cells}
                </tr>
            }
        })
        .collect_view();
    view! {
        <section class="bg-card overflow-hidden rounded-lg border shadow-[0_1px_2px_rgba(0,0,0,0.08)]">
            <div class="overflow-x-auto">
                <table class="w-full text-sm">
                    <caption class="px-4 pt-4 pb-3 text-left text-base font-semibold md:px-6 md:pt-5">
                        {by.label()}
                    </caption>
                    <thead class="border-y">
                        <tr>
                            <th scope="col" class=format!("{HEAD} bg-card sticky left-0 pl-4 text-left md:pl-6")>
                                {by.column()}
                            </th>
                            {headings}
                        </tr>
                    </thead>
                    <tbody class="divide-y">{rows}</tbody>
                </table>
            </div>
        </section>
    }
    .into_any()
}
