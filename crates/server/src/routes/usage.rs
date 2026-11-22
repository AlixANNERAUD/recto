//! The Usage administration page, what was printed and scanned over a range of days, and its
//! CSV exports. They sit behind the administration router's layers, so anyone but an
//! administrator gets 404.

use axum::{
    Extension, Router,
    extract::{Query, State},
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use leptos::prelude::provide_context;
use recto_app::{
    i18n, messages,
    pages::administration::usage::{
        Breakdown, PrintTotals, Report, ScanTotals, USAGE, UsagePageData, UsageRow, anonymous_label,
    },
    state::{Currency, User},
};
use recto_domain::reports::{self, Key, Range};
use serde::Deserialize;

use super::{administration::AdministrationState, render};
use crate::csv;

/// The query of the page and its exports.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct UsageQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub by: Option<String>,
}

impl UsageQuery {
    /// The query of a request; a malformed one reads as empty.
    pub(crate) fn of(uri: &Uri) -> Self {
        Query::<UsageQuery>::try_from_uri(uri)
            .map(|query| query.0)
            .unwrap_or_default()
    }

    pub(crate) fn breakdown(&self) -> Breakdown {
        Breakdown::from_keyword(self.by.as_deref().unwrap_or_default())
    }
}

/// `GET /administration/usage` and its exports `print-jobs.csv`, `scans.csv` and `totals.csv`.
pub(super) fn router() -> Router<AdministrationState> {
    let mut router = Router::new().route(USAGE, get(usage_page));
    for file in [Export::PrintJobs, Export::Scans, Export::Totals] {
        router = router.route(
            &format!("{USAGE}/{}.csv", file.keyword()),
            get(
                move |State(state): State<AdministrationState>,
                      Extension(actor): Extension<User>,
                      parts: Parts| export(state, actor, parts, file),
            ),
        );
    }
    router
}

/// The range a query asks for: this month without dates, the local days `from` to `to`
/// otherwise. Refused when only one date is given, a date does not read as `YYYY-MM-DD`, or the
/// range is not one [`Range::days`] accepts.
pub(crate) fn requested_range(
    query: &UsageQuery,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<(NaiveDate, NaiveDate, Range), ()> {
    // An empty field of the form is no date.
    fn given(date: &Option<String>) -> Option<&str> {
        date.as_deref().filter(|date| !date.trim().is_empty())
    }
    let (from, to) = match (given(&query.from), given(&query.to)) {
        (None, None) => reports::this_month_days(now, tz),
        (Some(from), Some(to)) => {
            let read = |date: &str| NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok();
            (read(from).ok_or(())?, read(to).ok_or(())?)
        }
        _ => return Err(()),
    };
    let range = Range::days(from, to, tz).ok_or(())?;
    Ok((from, to, range))
}

/// The breakdown the domain reads.
pub(crate) fn domain_breakdown(by: Breakdown) -> reports::Breakdown {
    match by {
        Breakdown::User => reports::Breakdown::User,
        Breakdown::Group => reports::Breakdown::Group,
        Breakdown::Printer => reports::Breakdown::Printer,
        Breakdown::Scanner => reports::Breakdown::Scanner,
    }
}

fn print_totals(totals: reports::PrintTotals) -> PrintTotals {
    PrintTotals {
        jobs: totals.jobs,
        pages: totals.pages,
        sheets: totals.sheets,
        color_pages: totals.color_pages,
        mono_pages: totals.mono_pages,
        cost: totals.cost,
        uncounted: totals.uncounted,
    }
}

fn scan_totals(totals: reports::ScanTotals) -> ScanTotals {
    ScanTotals {
        scans: totals.scans,
        pages: totals.pages,
        saved: totals.saved,
    }
}

fn usage_row(row: reports::Row) -> UsageRow {
    let (label, detail) = match row.key {
        Key::User {
            username,
            display_name,
            ..
        } => (display_name, Some(username)),
        Key::Anonymous => (anonymous_label().to_owned(), None),
        Key::Group { name, .. } => (name, None),
        Key::Printer(name) | Key::Scanner(name) => (name, None),
    };
    UsageRow {
        label,
        detail,
        print: row.print.map(print_totals),
        scan: row.scan.map(scan_totals),
    }
}

/// `GET /administration/usage?from=YYYY-MM-DD&to=YYYY-MM-DD&by=user|group|printer|scanner`.
async fn usage_page(State(state): State<AdministrationState>, parts: Parts) -> Response {
    let query = UsageQuery::of(&parts.uri);
    let by = query.breakdown();
    let now = Utc::now();
    let tz = state.app.timezone;
    let date = |date: NaiveDate| date.format("%Y-%m-%d").to_string();
    let (this_from, this_to) = reports::this_month_days(now, tz);
    let (last_from, last_to) = reports::last_month_days(now, tz);
    let mut data = UsagePageData {
        from: query.from.clone().unwrap_or_default(),
        to: query.to.clone().unwrap_or_default(),
        this_month: (date(this_from), date(this_to)),
        last_month: (date(last_from), date(last_to)),
        by,
        currency: state.app.currency.clone(),
        report: None,
        invalid_dates: false,
    };
    let status = match requested_range(&query, now, tz) {
        Err(()) => {
            data.invalid_dates = true;
            StatusCode::BAD_REQUEST
        }
        Ok((from, to, range)) => {
            data.from = date(from);
            data.to = date(to);
            let database = &state.app.database;
            let loaded = async {
                let print = reports::print_total(database, range).await?;
                let scan = reports::scan_total(database, range).await?;
                let rows = reports::rows(database, range, domain_breakdown(by)).await?;
                Ok::<_, recto_domain::Error>(Report {
                    print: print_totals(print),
                    scan: scan_totals(scan),
                    rows: rows.into_iter().map(usage_row).collect(),
                })
            }
            .await;
            match loaded {
                Ok(report) => {
                    data.report = Some(report);
                    StatusCode::OK
                }
                Err(error) => {
                    tracing::error!(%error, "loading the usage failed");
                    StatusCode::SERVICE_UNAVAILABLE
                }
            }
        }
    };
    render(&state.app, state.methods, parts, status, move || {
        provide_context(data.clone());
    })
    .await
}

/// The CSV files of the Usage page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Export {
    /// Every ledger row of the range.
    PrintJobs,
    /// Every scan logged in the range.
    Scans,
    /// The rows of the table the page shows.
    Totals,
}

impl Export {
    fn keyword(self) -> &'static str {
        match self {
            Export::PrintJobs => "print-jobs",
            Export::Scans => "scans",
            Export::Totals => "totals",
        }
    }
}

/// The header line of a file.
fn header_line(names: &[&str]) -> String {
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    csv::line(&names)
}

/// An optional count: empty when unknown.
fn count(value: Option<i64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

/// `GET /administration/usage/<file>.csv` with the page's query: the file, built in memory.
/// Answers 400 with the message as text when the range cannot be read, and 503 when the
/// database fails.
async fn export(state: AdministrationState, actor: User, parts: Parts, file: Export) -> Response {
    let locale = i18n::from_headers(&parts.headers);
    let query = UsageQuery::of(&parts.uri);
    let by = query.breakdown();
    let tz = state.app.timezone;
    let Ok((from, to, range)) = requested_range(&query, Utc::now(), tz) else {
        return (
            StatusCode::BAD_REQUEST,
            messages::invalid_usage_dates(locale),
        )
            .into_response();
    };
    let database = &state.app.database;
    let currency = &state.app.currency;
    let built = match file {
        Export::PrintJobs => print_jobs_csv(database, range, tz, currency).await,
        Export::Scans => scans_csv(database, range, tz).await,
        Export::Totals => totals_csv(database, range, by, currency).await,
    };
    let body = match built {
        Ok(body) => body,
        Err(error) => {
            tracing::error!(%error, "exporting the usage failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                messages::usage_unavailable(locale),
            )
                .into_response();
        }
    };
    let name = match file {
        Export::Totals => format!("usage-by-{}-{from}-{to}.csv", by.keyword()),
        _ => format!("{}-{from}-{to}.csv", file.keyword()),
    };
    // Personal data leaves recto: say who took which.
    tracing::info!(
        by = actor.username,
        export = name,
        from = %from,
        to = %to,
        "usage exported"
    );
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    // The name is made of recto's own words and dates, safe inside quotes.
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Every ledger row of the range, refunded ones included.
async fn print_jobs_csv(
    database: &sea_orm::DatabaseConnection,
    range: Range,
    tz: Tz,
    currency: &Currency,
) -> recto_domain::Result<String> {
    let mut body = String::from(csv::BOM);
    body.push_str(&header_line(&[
        "id",
        "submitted_at",
        "username",
        "printer",
        "document_name",
        "document_format",
        "document_pages",
        "copies",
        "sides",
        "color_mode",
        "pages",
        "sheets",
        "color_pages",
        "mono_pages",
        "cost",
        "currency",
        "state",
        "refunded",
        "cups_job_id",
    ]));
    for job in reports::print_entries(database, range).await? {
        body.push_str(&csv::line(&[
            job.id.to_string(),
            job.submitted_at.with_timezone(&tz).to_rfc3339(),
            csv::text(&job.username),
            csv::text(&job.printer),
            csv::text(&job.document_name),
            csv::text(job.document_format.as_deref().unwrap_or_default()),
            count(job.document_pages),
            job.copies.to_string(),
            csv::text(job.sides.as_deref().unwrap_or_default()),
            csv::text(job.color_mode.as_deref().unwrap_or_default()),
            count(job.pages),
            count(job.sheets),
            count(job.color_pages),
            count(job.mono_pages),
            currency.amount(job.cost),
            csv::text(&currency.code),
            csv::text(&job.state),
            job.refunded.to_string(),
            count(job.cups_job_id),
        ]));
    }
    Ok(body)
}

/// Every scan logged in the range.
async fn scans_csv(
    database: &sea_orm::DatabaseConnection,
    range: Range,
    tz: Tz,
) -> recto_domain::Result<String> {
    let mut body = String::from(csv::BOM);
    body.push_str(&header_line(&[
        "id",
        "scanned_at",
        "source",
        "username",
        "scanner",
        "pages",
        "format",
        "saved",
    ]));
    for scan in reports::scan_entries(database, range).await? {
        body.push_str(&csv::line(&[
            scan.id.to_string(),
            scan.scanned_at.with_timezone(&tz).to_rfc3339(),
            csv::text(&scan.source),
            csv::text(&scan.username),
            csv::text(&scan.scanner),
            scan.pages.to_string(),
            csv::text(&scan.format),
            scan.saved.to_string(),
        ]));
    }
    Ok(body)
}

/// The rows of the table `by`, in the page's order, without a total line, which would get in
/// the way of a pivot.
async fn totals_csv(
    database: &sea_orm::DatabaseConnection,
    range: Range,
    by: Breakdown,
    currency: &Currency,
) -> recto_domain::Result<String> {
    const PRINT: [&str; 8] = [
        "jobs",
        "pages",
        "sheets",
        "color_pages",
        "mono_pages",
        "cost",
        "currency",
        "uncounted_jobs",
    ];
    const SCAN: [&str; 3] = ["scans", "scanned_pages", "saved_scans"];
    let names: &[&str] = match by {
        Breakdown::User => &["username", "display_name"],
        Breakdown::Group => &["group"],
        Breakdown::Printer => &["printer"],
        Breakdown::Scanner => &["scanner"],
    };
    let mut columns: Vec<&str> = names.to_vec();
    if by != Breakdown::Scanner {
        columns.extend(PRINT);
    }
    if by != Breakdown::Printer {
        columns.extend(SCAN);
    }
    let mut body = String::from(csv::BOM);
    body.push_str(&header_line(&columns));
    for row in reports::rows(database, range, domain_breakdown(by)).await? {
        let mut cells = match &row.key {
            Key::User {
                username,
                display_name,
                ..
            } => vec![csv::text(username), csv::text(display_name)],
            Key::Anonymous => vec![String::new(), "anonymous".to_owned()],
            Key::Group { name, .. } | Key::Printer(name) | Key::Scanner(name) => {
                vec![csv::text(name)]
            }
        };
        if let Some(print) = row.print {
            cells.extend([
                print.jobs.to_string(),
                print.pages.to_string(),
                print.sheets.to_string(),
                print.color_pages.to_string(),
                print.mono_pages.to_string(),
                currency.amount(print.cost),
                csv::text(&currency.code),
                print.uncounted.to_string(),
            ]);
        }
        if let Some(scan) = row.scan {
            cells.extend([
                scan.scans.to_string(),
                scan.pages.to_string(),
                scan.saved.to_string(),
            ]);
        }
        body.push_str(&csv::line(&cells));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(from: Option<&str>, to: Option<&str>) -> UsageQuery {
        UsageQuery {
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
            by: None,
        }
    }

    #[test]
    fn ranges_are_read_from_the_query() {
        let now: DateTime<Utc> = "2026-11-17T10:00:00Z".parse().unwrap();
        let tz = chrono_tz::UTC;
        let (from, to, _) = requested_range(&query(None, None), now, tz).unwrap();
        assert_eq!(
            (from.to_string(), to.to_string()),
            ("2026-11-01".into(), "2026-11-30".into())
        );
        let (from, to, _) = requested_range(&query(Some(""), Some(" ")), now, tz).unwrap();
        assert_eq!(from.to_string(), "2026-11-01");
        assert_eq!(to.to_string(), "2026-11-30");
        assert!(requested_range(&query(Some("2026-10-01"), Some("2026-10-31")), now, tz).is_ok());
        for (from, to) in [
            (Some("2026-13-01"), Some("2026-12-01")),
            (Some("2026-11-02"), Some("2026-11-01")),
            (Some("2026-11-01"), None),
            (None, Some("2026-11-01")),
            (Some("1999-12-31"), Some("2026-01-01")),
            (Some("2026-01-01"), Some("3000-01-01")),
            (Some("9999-12-31"), Some("9999-12-31")),
            (Some("+12026-01-01"), Some("+12026-01-02")),
            (Some("yesterday"), Some("today")),
        ] {
            assert!(
                requested_range(&query(from, to), now, tz).is_err(),
                "{from:?} {to:?}"
            );
        }
    }
}
