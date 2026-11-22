//! Usage reports: what was printed and scanned over a range of days, in total and by user,
//! group, printer or scanner, from the print ledger and the scan log.
//!
//! Print totals leave out refunded jobs, which printed nothing, and count the jobs still
//! pending. A job whose counts are unknown adds nothing to the page metrics and counts as
//! uncounted. Groups count their current members, as recorded at each member's last sign-in,
//! so a person in two groups counts in both and group rows need not add up to the total.
//!
//! Every query runs on SQLite and PostgreSQL alike: sums are cast back to 64-bit integers,
//! booleans are summed through `CASE`, and every selected column is grouped.

use std::{cmp::Reverse, collections::HashMap, ops::RangeInclusive};

use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, Utc};
use chrono_tz::Tz;
use recto_entity::{group_memberships, groups, print_jobs, scan_jobs, users};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QueryResult,
    sea_query::{Alias, Asterisk, CaseStatement, Expr, ExprTrait, Func, Query, SelectStatement},
};

use crate::{
    error::Result,
    quotas::{midnight, sum},
};

/// The years a range may start and end in. Beyond them a date is a typo, and refusing them
/// keeps every bound far from chrono's limits and in the four-digit years that SQLite, which
/// compares timestamps as text, orders correctly.
pub const YEARS: RangeInclusive<i32> = 2000..=2999;

/// A span of time, `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl Range {
    /// The local days `from` to `to`, both included, in `tz`: from midnight of `from` to
    /// midnight of the day after `to`. `None` when `to` is before `from` or a year is outside
    /// [`YEARS`].
    pub fn days(from: NaiveDate, to: NaiveDate, tz: Tz) -> Option<Range> {
        if to < from || !YEARS.contains(&from.year()) || !YEARS.contains(&to.year()) {
            return None;
        }
        Some(Range {
            start: midnight(tz, from),
            end: midnight(tz, to + Duration::days(1)),
        })
    }
}

/// The first and last local days of the month that contains `now`.
pub fn this_month_days(now: DateTime<Utc>, tz: Tz) -> (NaiveDate, NaiveDate) {
    let today = now.with_timezone(&tz).date_naive();
    let first = today.with_day(1).unwrap_or(today);
    let next = first.checked_add_months(Months::new(1)).unwrap_or(first);
    (first, next.pred_opt().unwrap_or(first))
}

/// The first and last local days of the month before the one that contains `now`.
pub fn last_month_days(now: DateTime<Utc>, tz: Tz) -> (NaiveDate, NaiveDate) {
    let (first, _) = this_month_days(now, tz);
    let previous = first.checked_sub_months(Months::new(1)).unwrap_or(first);
    (previous, first.pred_opt().unwrap_or(first))
}

/// What was printed, from the ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrintTotals {
    pub jobs: i64,
    pub pages: i64,
    pub sheets: i64,
    pub color_pages: i64,
    pub mono_pages: i64,
    /// In minor currency units.
    pub cost: i64,
    /// Jobs whose counts are unknown, which add nothing to the page metrics.
    pub uncounted: i64,
}

/// What was scanned, from the scan log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanTotals {
    pub scans: i64,
    pub pages: i64,
    /// Scans saved to the output folder.
    pub saved: i64,
}

/// What a report's rows are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breakdown {
    User,
    Group,
    Printer,
    Scanner,
}

/// What one row of a report is about.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    User {
        id: i64,
        username: String,
        display_name: String,
    },
    /// Scans without a user: anonymous scans from the network.
    Anonymous,
    Group {
        id: i64,
        name: String,
    },
    Printer(String),
    Scanner(String),
}

impl Key {
    /// What rows are ordered by after their counts.
    fn name(&self) -> (bool, String) {
        match self {
            Key::User {
                username,
                display_name,
                ..
            } => (
                false,
                format!("{display_name}\u{0}{username}").to_lowercase(),
            ),
            // After every named row.
            Key::Anonymous => (true, String::new()),
            Key::Group { name, .. } => (false, name.to_lowercase()),
            Key::Printer(name) | Key::Scanner(name) => (false, name.to_lowercase()),
        }
    }
}

/// Sorts rows by `pages` descending, then by name.
fn sorted<T>(mut rows: Vec<(Key, T)>, pages: impl Fn(&T) -> i64) -> Vec<(Key, T)> {
    rows.sort_by_cached_key(|(key, totals)| (Reverse(pages(totals)), key.name()));
    rows
}

/// `CAST(SUM(column) AS BIGINT)` of a column of `table`.
fn sum_of(table: impl sea_orm::Iden + 'static, column: impl sea_orm::Iden + 'static) -> Expr {
    sum(Expr::col((table, column)))
}

/// The print totals columns, selected on `print_jobs` rows in the range that were not refunded.
fn print_query(range: Range) -> SelectStatement {
    let jobs = print_jobs::Entity;
    let mut query = Query::select();
    query
        .from(jobs)
        .expr_as(
            Func::count(Expr::col((jobs, print_jobs::Column::Id))),
            Alias::new("jobs"),
        )
        .expr_as(sum_of(jobs, print_jobs::Column::Pages), Alias::new("pages"))
        .expr_as(
            sum_of(jobs, print_jobs::Column::Sheets),
            Alias::new("sheets"),
        )
        .expr_as(
            sum_of(jobs, print_jobs::Column::ColorPages),
            Alias::new("color_pages"),
        )
        .expr_as(
            sum_of(jobs, print_jobs::Column::MonoPages),
            Alias::new("mono_pages"),
        )
        .expr_as(sum_of(jobs, print_jobs::Column::Cost), Alias::new("cost"))
        .expr_as(
            Func::count(Expr::col(Asterisk))
                .sub(Func::count(Expr::col((jobs, print_jobs::Column::Pages)))),
            Alias::new("uncounted"),
        )
        .and_where(Expr::col((jobs, print_jobs::Column::Refunded)).eq(false))
        .and_where(
            Expr::col((jobs, print_jobs::Column::SubmittedAt)).gte(range.start.fixed_offset()),
        )
        .and_where(Expr::col((jobs, print_jobs::Column::SubmittedAt)).lt(range.end.fixed_offset()));
    query
}

/// The scan totals columns, selected on `scan_jobs` rows in the range.
fn scan_query(range: Range) -> SelectStatement {
    let scans = scan_jobs::Entity;
    let saved = CaseStatement::new()
        .case(Expr::col((scans, scan_jobs::Column::Saved)).eq(true), 1)
        .finally(0);
    let mut query = Query::select();
    query
        .from(scans)
        .expr_as(
            Func::count(Expr::col((scans, scan_jobs::Column::Id))),
            Alias::new("scans"),
        )
        .expr_as(sum_of(scans, scan_jobs::Column::Pages), Alias::new("pages"))
        .expr_as(
            Func::cast_as(Func::sum(Expr::Case(Box::new(saved))), Alias::new("BIGINT")),
            Alias::new("saved"),
        )
        .and_where(Expr::col((scans, scan_jobs::Column::ScannedAt)).gte(range.start.fixed_offset()))
        .and_where(Expr::col((scans, scan_jobs::Column::ScannedAt)).lt(range.end.fixed_offset()));
    query
}

fn count(row: &QueryResult, column: &str) -> Result<i64> {
    Ok(row.try_get::<Option<i64>>("", column)?.unwrap_or(0))
}

fn print_totals(row: &QueryResult) -> Result<PrintTotals> {
    Ok(PrintTotals {
        jobs: count(row, "jobs")?,
        pages: count(row, "pages")?,
        sheets: count(row, "sheets")?,
        color_pages: count(row, "color_pages")?,
        mono_pages: count(row, "mono_pages")?,
        cost: count(row, "cost")?,
        uncounted: count(row, "uncounted")?,
    })
}

fn scan_totals(row: &QueryResult) -> Result<ScanTotals> {
    Ok(ScanTotals {
        scans: count(row, "scans")?,
        pages: count(row, "pages")?,
        saved: count(row, "saved")?,
    })
}

/// What was printed in the range.
pub async fn print_total(db: &impl ConnectionTrait, range: Range) -> Result<PrintTotals> {
    match db.query_one(&print_query(range)).await? {
        Some(row) => print_totals(&row),
        None => Ok(PrintTotals::default()),
    }
}

/// What was scanned in the range, anonymous scans included.
pub async fn scan_total(db: &impl ConnectionTrait, range: Range) -> Result<ScanTotals> {
    match db.query_one(&scan_query(range)).await? {
        Some(row) => scan_totals(&row),
        None => Ok(ScanTotals::default()),
    }
}

/// Selects and groups by the user of `table`'s `user_id`, joined to `users` (left, so that
/// rows without a user stay).
fn by_user(
    query: &mut SelectStatement,
    table: impl sea_orm::Iden + Copy + 'static,
    user_id: impl sea_orm::Iden + Copy + 'static,
) {
    query
        .left_join(
            users::Entity,
            Expr::col((users::Entity, users::Column::Id)).equals((table, user_id)),
        )
        .expr_as(Expr::col((table, user_id)), Alias::new("key_id"))
        .expr_as(
            Expr::col((users::Entity, users::Column::Username)),
            Alias::new("key_name"),
        )
        .expr_as(
            Expr::col((users::Entity, users::Column::DisplayName)),
            Alias::new("key_detail"),
        )
        .group_by_col((table, user_id))
        .group_by_col((users::Entity, users::Column::Username))
        .group_by_col((users::Entity, users::Column::DisplayName));
}

/// Selects and groups by each current group of `table`'s user; rows without a group drop out.
fn by_group(
    query: &mut SelectStatement,
    table: impl sea_orm::Iden + Copy + 'static,
    user_id: impl sea_orm::Iden + Copy + 'static,
) {
    query
        .inner_join(
            group_memberships::Entity,
            Expr::col((group_memberships::Entity, group_memberships::Column::UserId))
                .equals((table, user_id)),
        )
        .inner_join(
            groups::Entity,
            Expr::col((groups::Entity, groups::Column::Id)).equals((
                group_memberships::Entity,
                group_memberships::Column::GroupId,
            )),
        )
        .expr_as(
            Expr::col((groups::Entity, groups::Column::Id)),
            Alias::new("key_id"),
        )
        .expr_as(
            Expr::col((groups::Entity, groups::Column::Name)),
            Alias::new("key_name"),
        )
        .group_by_col((groups::Entity, groups::Column::Id))
        .group_by_col((groups::Entity, groups::Column::Name));
}

/// Selects and groups by a device column of `table`.
fn by_device(
    query: &mut SelectStatement,
    table: impl sea_orm::Iden + Copy + 'static,
    device: impl sea_orm::Iden + Copy + 'static,
) {
    query
        .expr_as(Expr::col((table, device)), Alias::new("key_name"))
        .group_by_col((table, device));
}

/// The key of a grouped row.
fn key(row: &QueryResult, by: Breakdown) -> Result<Key> {
    Ok(match by {
        Breakdown::User => match row.try_get::<Option<i64>>("", "key_id")? {
            Some(id) => Key::User {
                id,
                username: row
                    .try_get::<Option<String>>("", "key_name")?
                    .unwrap_or_default(),
                display_name: row
                    .try_get::<Option<String>>("", "key_detail")?
                    .unwrap_or_default(),
            },
            None => Key::Anonymous,
        },
        Breakdown::Group => Key::Group {
            id: row.try_get("", "key_id")?,
            name: row.try_get("", "key_name")?,
        },
        Breakdown::Printer => Key::Printer(row.try_get("", "key_name")?),
        Breakdown::Scanner => Key::Scanner(row.try_get("", "key_name")?),
    })
}

/// What was printed in the range, one row per user, group or printer, heaviest first. Empty for
/// [`Breakdown::Scanner`].
pub async fn printing(
    db: &impl ConnectionTrait,
    range: Range,
    by: Breakdown,
) -> Result<Vec<(Key, PrintTotals)>> {
    let mut query = print_query(range);
    let table = print_jobs::Entity;
    match by {
        Breakdown::User => by_user(&mut query, table, print_jobs::Column::UserId),
        Breakdown::Group => by_group(&mut query, table, print_jobs::Column::UserId),
        Breakdown::Printer => by_device(&mut query, table, print_jobs::Column::Printer),
        Breakdown::Scanner => return Ok(Vec::new()),
    }
    let rows = db.query_all(&query).await?;
    let rows = rows
        .iter()
        .map(|row| Ok((key(row, by)?, print_totals(row)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(sorted(rows, |totals| totals.pages))
}

/// What was scanned in the range, one row per user (anonymous scans in one row), group or
/// scanner, heaviest first. Empty for [`Breakdown::Printer`].
pub async fn scanning(
    db: &impl ConnectionTrait,
    range: Range,
    by: Breakdown,
) -> Result<Vec<(Key, ScanTotals)>> {
    let mut query = scan_query(range);
    let table = scan_jobs::Entity;
    match by {
        Breakdown::User => by_user(&mut query, table, scan_jobs::Column::UserId),
        Breakdown::Group => by_group(&mut query, table, scan_jobs::Column::UserId),
        Breakdown::Scanner => by_device(&mut query, table, scan_jobs::Column::Scanner),
        Breakdown::Printer => return Ok(Vec::new()),
    }
    let rows = db.query_all(&query).await?;
    let rows = rows
        .iter()
        .map(|row| Ok((key(row, by)?, scan_totals(row)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(sorted(rows, |totals| totals.pages))
}

/// One row of a report's table: what its key printed and scanned. A user or group row has
/// both, zero when they did only one; a printer row only prints, a scanner row only scans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: Key,
    pub print: Option<PrintTotals>,
    pub scan: Option<ScanTotals>,
}

/// The rows of the table `by` for the range: [`printing`] and [`scanning`] merged by key,
/// sorted by printed pages, then scanned pages, descending, then by name.
pub async fn rows(db: &impl ConnectionTrait, range: Range, by: Breakdown) -> Result<Vec<Row>> {
    let printed = printing(db, range, by).await?;
    let scanned = scanning(db, range, by).await?;
    let both = matches!(by, Breakdown::User | Breakdown::Group);
    let mut rows: Vec<Row> = printed
        .into_iter()
        .map(|(key, print)| Row {
            key,
            print: Some(print),
            scan: both.then(ScanTotals::default),
        })
        .collect();
    // Indexed, so that thousands of users who both print and scan merge in linear time.
    let index: HashMap<Key, usize> = rows
        .iter()
        .enumerate()
        .map(|(position, row)| (row.key.clone(), position))
        .collect();
    for (key, scan) in scanned {
        match index.get(&key) {
            Some(&position) => rows[position].scan = Some(scan),
            None => rows.push(Row {
                key,
                print: both.then(PrintTotals::default),
                scan: Some(scan),
            }),
        }
    }
    rows.sort_by_cached_key(|row| {
        (
            Reverse(row.print.map_or(0, |print| print.pages)),
            Reverse(row.scan.map_or(0, |scan| scan.pages)),
            row.key.name(),
        )
    });
    Ok(rows)
}

/// Every ledger row of the range, refunded ones included, oldest first.
pub async fn print_entries(
    db: &impl ConnectionTrait,
    range: Range,
) -> Result<Vec<print_jobs::Model>> {
    Ok(print_jobs::Entity::find()
        .filter(print_jobs::Column::SubmittedAt.gte(range.start.fixed_offset()))
        .filter(print_jobs::Column::SubmittedAt.lt(range.end.fixed_offset()))
        .order_by_asc(print_jobs::Column::SubmittedAt)
        .order_by_asc(print_jobs::Column::Id)
        .all(db)
        .await?)
}

/// Every scan logged in the range, oldest first.
pub async fn scan_entries(
    db: &impl ConnectionTrait,
    range: Range,
) -> Result<Vec<scan_jobs::Model>> {
    Ok(scan_jobs::Entity::find()
        .filter(scan_jobs::Column::ScannedAt.gte(range.start.fixed_offset()))
        .filter(scan_jobs::Column::ScannedAt.lt(range.end.fixed_offset()))
        .order_by_asc(scan_jobs::Column::ScannedAt)
        .order_by_asc(scan_jobs::Column::Id)
        .all(db)
        .await?)
}

#[cfg(test)]
mod tests;
