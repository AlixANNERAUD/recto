//! The Usage administration page: what was printed and scanned over a range of days. It sits
//! behind the administration router's layers, so anyone but an administrator gets 404.

use axum::{
    Router,
    extract::{Query, State},
    http::{StatusCode, Uri, request::Parts},
    response::Response,
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use leptos::prelude::provide_context;
use recto_app::pages::administration::usage::{
    Breakdown, PrintTotals, Report, ScanTotals, USAGE, UsagePageData, UsageRow, anonymous_label,
};
use recto_domain::reports::{self, Key, Range};
use serde::Deserialize;

use super::{administration::AdministrationState, render};

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

/// `GET /administration/usage` and, later, its exports.
pub(super) fn router() -> Router<AdministrationState> {
    Router::new().route(USAGE, get(usage_page))
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
