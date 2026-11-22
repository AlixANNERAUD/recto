//! The reports on a database, in Paris time.

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::{Europe::Paris, Tz};
use recto_entity::groups as group_rows;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use super::*;
use crate::{
    ledger::{self, NewEntry, Prices},
    scans::{self, NewScan, Source},
    users::{Identity, sign_in_with_identity},
};

const TZ: Tz = Paris;

fn utc(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

fn date(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

/// November 2026 in Paris.
fn november() -> Range {
    Range::days(date("2026-11-01"), date("2026-11-30"), TZ).unwrap()
}

/// Signs a user in with these groups; returns their id.
async fn person(db: &DatabaseConnection, name: &str, groups: &[&str]) -> i64 {
    sign_in_with_identity(
        db,
        &Identity {
            issuer: "https://id.example".to_owned(),
            subject: name.to_owned(),
            preferred_username: Some(name.to_owned()),
            groups: Some(groups.iter().map(|group| (*group).to_owned()).collect()),
            ..Identity::default()
        },
    )
    .await
    .unwrap()
    .id
}

async fn group_id(db: &DatabaseConnection, name: &str) -> i64 {
    group_rows::Entity::find()
        .filter(group_rows::Column::Name.eq(name))
        .one(db)
        .await
        .unwrap()
        .expect("the group")
        .id
}

/// Black and white pages cost 5, color pages 20.
const PRICES: Prices = Prices {
    mono_page: 5,
    color_page: 20,
    sheet: 0,
};

/// Enters a job of `pages` pages (`None`: unknown) in the ledger at `at`; returns its id.
async fn printed(
    db: &DatabaseConnection,
    user_id: i64,
    printer: &str,
    pages: Option<u32>,
    color: bool,
    at: &str,
) -> i64 {
    ledger::insert(
        db,
        NewEntry {
            user_id,
            username: String::new(),
            printer: printer.to_owned(),
            document_name: "a.pdf".to_owned(),
            document_format: "application/pdf".to_owned(),
            document_pages: pages,
            copies: 1,
            sides: "one-sided".to_owned(),
            color_mode: if color { "color" } else { "monochrome" }.to_owned(),
            counts: ledger::counts(pages, 1, false, color, PRICES),
        },
        utc(at),
    )
    .await
    .unwrap()
}

/// Logs a scan of `pages` pages at `at`, saved or not; `None` is an anonymous network scan.
async fn scanned(
    db: &DatabaseConnection,
    user: Option<(i64, &str)>,
    scanner: &str,
    pages: u32,
    saved: bool,
    at: &str,
) -> i64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let result_id = user.map(|_| format!("result-{number}"));
    let id = scans::record(
        db,
        NewScan {
            user_id: user.map(|(id, _)| id),
            username: user.map(|(_, name)| name.to_owned()).unwrap_or_default(),
            scanner: scanner.to_owned(),
            result_id: result_id.clone(),
            source: if user.is_some() {
                Source::Web
            } else {
                Source::Escl
            },
            format: "pdf".to_owned(),
            pages,
        },
        utc(at),
    )
    .await
    .unwrap();
    if saved {
        let (user_id, _) = user.unwrap();
        assert!(
            scans::mark_saved(db, user_id, &result_id.unwrap())
                .await
                .unwrap()
        );
    }
    id
}

fn user(id: i64, name: &str) -> Key {
    Key::User {
        id,
        username: name.to_owned(),
        display_name: name.to_owned(),
    }
}

#[test]
fn ranges_follow_the_local_calendar() {
    let now = utc("2026-11-17T10:00:00Z");
    assert_eq!(
        Range::this_month(now, TZ),
        Range {
            start: utc("2026-10-31T23:00:00Z"),
            end: utc("2026-11-30T23:00:00Z"),
        }
    );
    assert_eq!(
        Range::last_month(now, TZ),
        Range {
            start: utc("2026-09-30T22:00:00Z"),
            end: utc("2026-10-31T23:00:00Z"),
        }
    );
    assert_eq!(
        this_month_days(now, TZ),
        (date("2026-11-01"), date("2026-11-30"))
    );
    assert_eq!(
        last_month_days(now, TZ),
        (date("2026-10-01"), date("2026-10-31"))
    );
    // January's last month is December of the year before.
    assert_eq!(
        last_month_days(utc("2027-01-10T10:00:00Z"), TZ),
        (date("2026-12-01"), date("2026-12-31"))
    );
    // The day clocks go forward lasts 23 hours.
    assert_eq!(
        Range::days(date("2026-03-29"), date("2026-03-29"), TZ),
        Some(Range {
            start: utc("2026-03-28T23:00:00Z"),
            end: utc("2026-03-29T22:00:00Z"),
        })
    );
    assert_eq!(
        Range::days(date("2026-11-02"), date("2026-11-01"), TZ),
        None
    );
    assert!(Range::days(date("2000-01-01"), date("2999-12-31"), TZ).is_some());
    assert_eq!(
        Range::days(date("1999-12-31"), date("2026-01-01"), TZ),
        None
    );
    assert_eq!(
        Range::days(date("2026-01-01"), date("3000-01-01"), TZ),
        None
    );
    assert_eq!(
        Range::days(date("9999-12-31"), date("9999-12-31"), TZ),
        None
    );
}

#[tokio::test]
async fn print_totals_by_user_group_and_printer() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &["design", "staff"]).await;
    let bob = person(&db, "bob", &["design"]).await;
    let carol = person(&db, "carol", &[]).await;
    let design = group_id(&db, "design").await;
    let staff = group_id(&db, "staff").await;
    printed(&db, alice, "lab", Some(3), true, "2026-11-10T10:00:00Z").await;
    printed(&db, alice, "office", Some(2), false, "2026-11-11T10:00:00Z").await;
    printed(&db, bob, "office", Some(4), false, "2026-11-12T10:00:00Z").await;
    let refunded = printed(&db, bob, "office", Some(10), false, "2026-11-12T11:00:00Z").await;
    ledger::refund(&db, refunded).await.unwrap();
    printed(&db, bob, "office", Some(7), false, "2026-10-31T12:00:00Z").await;
    printed(&db, carol, "office", None, false, "2026-11-13T10:00:00Z").await;
    let range = november();

    let by_user = printing(&db, range, Breakdown::User).await.unwrap();
    assert_eq!(
        by_user,
        vec![
            (
                user(alice, "alice"),
                PrintTotals {
                    jobs: 2,
                    pages: 5,
                    sheets: 5,
                    color_pages: 3,
                    mono_pages: 2,
                    cost: 70,
                    uncounted: 0,
                }
            ),
            (
                user(bob, "bob"),
                PrintTotals {
                    jobs: 1,
                    pages: 4,
                    sheets: 4,
                    color_pages: 0,
                    mono_pages: 4,
                    cost: 20,
                    uncounted: 0,
                }
            ),
            (
                user(carol, "carol"),
                PrintTotals {
                    jobs: 1,
                    uncounted: 1,
                    ..PrintTotals::default()
                }
            ),
        ]
    );

    let by_group = printing(&db, range, Breakdown::Group).await.unwrap();
    let rows: Vec<(Key, i64, i64)> = by_group
        .into_iter()
        .map(|(key, totals)| (key, totals.pages, totals.jobs))
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                Key::Group {
                    id: design,
                    name: "design".to_owned()
                },
                9,
                3
            ),
            (
                Key::Group {
                    id: staff,
                    name: "staff".to_owned()
                },
                5,
                2
            ),
        ]
    );

    let by_printer = printing(&db, range, Breakdown::Printer).await.unwrap();
    assert_eq!(by_printer.len(), 2);
    assert_eq!(by_printer[0].0, Key::Printer("office".to_owned()));
    assert_eq!(
        (
            by_printer[0].1.pages,
            by_printer[0].1.jobs,
            by_printer[0].1.uncounted
        ),
        (6, 3, 1)
    );
    assert_eq!(by_printer[1].0, Key::Printer("lab".to_owned()));
    assert_eq!((by_printer[1].1.pages, by_printer[1].1.color_pages), (3, 3));

    assert!(
        printing(&db, range, Breakdown::Scanner)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        print_total(&db, range).await.unwrap(),
        PrintTotals {
            jobs: 4,
            pages: 9,
            sheets: 9,
            color_pages: 3,
            mono_pages: 6,
            cost: 90,
            uncounted: 1,
        }
    );
}

#[tokio::test]
async fn range_edges_count_local_days() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &[]).await;
    // 23:30 on 30 November in Paris counts, midnight on 1 December does not, midnight on
    // 1 November does.
    printed(&db, alice, "office", Some(1), false, "2026-11-30T22:30:00Z").await;
    printed(
        &db,
        alice,
        "office",
        Some(10),
        false,
        "2026-11-30T23:00:00Z",
    )
    .await;
    printed(
        &db,
        alice,
        "office",
        Some(100),
        false,
        "2026-10-31T23:00:00Z",
    )
    .await;
    printed(
        &db,
        alice,
        "office",
        Some(1000),
        false,
        "2026-10-31T22:59:59Z",
    )
    .await;
    let total = print_total(&db, november()).await.unwrap();
    assert_eq!((total.jobs, total.pages), (2, 101));
}

#[tokio::test]
async fn scan_totals_by_user_group_and_scanner() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &["design", "staff"]).await;
    let bob = person(&db, "bob", &["design"]).await;
    let design = group_id(&db, "design").await;
    let staff = group_id(&db, "staff").await;
    let at = "2026-11-10T10:00:00Z";
    scanned(&db, Some((alice, "alice")), "mock:0", 3, true, at).await;
    scanned(&db, Some((alice, "alice")), "mock:0", 1, false, at).await;
    scanned(&db, Some((bob, "bob")), "net:b", 5, false, at).await;
    scanned(&db, None, "mock:0", 2, false, at).await;
    // Outside the range.
    scanned(
        &db,
        Some((bob, "bob")),
        "net:b",
        50,
        false,
        "2026-12-01T10:00:00Z",
    )
    .await;
    let range = november();

    assert_eq!(
        scanning(&db, range, Breakdown::User).await.unwrap(),
        vec![
            (
                user(bob, "bob"),
                ScanTotals {
                    scans: 1,
                    pages: 5,
                    saved: 0
                }
            ),
            (
                user(alice, "alice"),
                ScanTotals {
                    scans: 2,
                    pages: 4,
                    saved: 1
                }
            ),
            (
                Key::Anonymous,
                ScanTotals {
                    scans: 1,
                    pages: 2,
                    saved: 0
                }
            ),
        ]
    );
    assert_eq!(
        scanning(&db, range, Breakdown::Group).await.unwrap(),
        vec![
            (
                Key::Group {
                    id: design,
                    name: "design".to_owned()
                },
                ScanTotals {
                    scans: 3,
                    pages: 9,
                    saved: 1
                }
            ),
            (
                Key::Group {
                    id: staff,
                    name: "staff".to_owned()
                },
                ScanTotals {
                    scans: 2,
                    pages: 4,
                    saved: 1
                }
            ),
        ]
    );
    assert_eq!(
        scanning(&db, range, Breakdown::Scanner).await.unwrap(),
        vec![
            (
                Key::Scanner("mock:0".to_owned()),
                ScanTotals {
                    scans: 3,
                    pages: 6,
                    saved: 1
                }
            ),
            (
                Key::Scanner("net:b".to_owned()),
                ScanTotals {
                    scans: 1,
                    pages: 5,
                    saved: 0
                }
            ),
        ]
    );
    assert!(
        scanning(&db, range, Breakdown::Printer)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        scan_total(&db, range).await.unwrap(),
        ScanTotals {
            scans: 4,
            pages: 11,
            saved: 1
        }
    );
}

#[tokio::test]
async fn entries_list_the_range_with_refunds() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &[]).await;
    let later = printed(&db, alice, "office", Some(1), false, "2026-11-20T10:00:00Z").await;
    let refunded = printed(&db, alice, "office", Some(2), false, "2026-11-05T10:00:00Z").await;
    ledger::refund(&db, refunded).await.unwrap();
    printed(&db, alice, "office", Some(3), false, "2026-10-31T12:00:00Z").await;
    let entries = print_entries(&db, november()).await.unwrap();
    let ids: Vec<i64> = entries.iter().map(|entry| entry.id).collect();
    assert_eq!(ids, vec![refunded, later]);
    assert!(entries[0].refunded);

    let second = scanned(&db, None, "mock:0", 1, false, "2026-11-21T10:00:00Z").await;
    let first = scanned(
        &db,
        Some((alice, "alice")),
        "mock:0",
        2,
        true,
        "2026-11-02T10:00:00Z",
    )
    .await;
    scanned(&db, None, "mock:0", 1, false, "2026-12-01T10:00:00Z").await;
    let scans: Vec<i64> = scan_entries(&db, november())
        .await
        .unwrap()
        .iter()
        .map(|scan| scan.id)
        .collect();
    assert_eq!(scans, vec![first, second]);
}

#[tokio::test]
async fn rows_merge_printing_and_scanning() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &["design"]).await;
    let bob = person(&db, "bob", &["design"]).await;
    let at = "2026-11-10T10:00:00Z";
    printed(&db, alice, "office", Some(2), false, at).await;
    scanned(&db, Some((bob, "bob")), "mock:0", 3, true, at).await;
    scanned(&db, None, "mock:0", 1, false, at).await;
    let range = november();

    let rows = rows(&db, range, Breakdown::User).await.unwrap();
    let keys: Vec<&Key> = rows.iter().map(|row| &row.key).collect();
    assert_eq!(
        keys,
        vec![&user(alice, "alice"), &user(bob, "bob"), &Key::Anonymous]
    );
    assert_eq!(rows[0].print.unwrap().pages, 2);
    assert_eq!(rows[0].scan, Some(ScanTotals::default()));
    // Someone who only scanned still has a row.
    assert_eq!(rows[1].print, Some(PrintTotals::default()));
    assert_eq!(rows[1].scan.unwrap().saved, 1);

    let printers = super::rows(&db, range, Breakdown::Printer).await.unwrap();
    assert_eq!(printers.len(), 1);
    assert_eq!(printers[0].scan, None);
    let scanners = super::rows(&db, range, Breakdown::Scanner).await.unwrap();
    assert_eq!(scanners.len(), 1);
    assert_eq!(scanners[0].print, None);
    assert_eq!(scanners[0].scan.unwrap().pages, 4);
}
