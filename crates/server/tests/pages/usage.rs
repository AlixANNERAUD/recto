//! Usage: the scan log, the Usage administration page and its CSV exports.

use chrono::{DateTime, Datelike, Months, Utc};
use recto_domain::{
    ledger::{self, NewEntry, Prices},
    scans::{self, NewScan, Source},
};
use recto_entity::scan_jobs;
use sea_orm::ConnectionTrait;

use super::*;

impl Harness {
    async fn user_id(&self, username: &str) -> i64 {
        users::find_by_username(&self.database, username)
            .await
            .unwrap()
            .expect("a user")
            .id
    }

    /// Enters a job in the ledger at `at`; returns its id. 33 minor units a page in color, 5 in
    /// black and white; `None` pages are unknown.
    async fn printed(
        &self,
        username: &str,
        printer: &str,
        document_name: &str,
        pages: Option<u32>,
        color: bool,
        at: &str,
    ) -> i64 {
        let user_id = self.user_id(username).await;
        let prices = Prices {
            mono_page: 5,
            color_page: 33,
            sheet: 0,
        };
        ledger::insert(
            &self.database,
            NewEntry {
                user_id,
                username: username.into(),
                printer: printer.into(),
                document_name: document_name.into(),
                document_format: "application/pdf".into(),
                document_pages: pages,
                copies: 1,
                sides: "one-sided".into(),
                color_mode: if color { "color" } else { "monochrome" }.into(),
                counts: ledger::counts(pages, 1, false, color, prices),
            },
            at.parse::<DateTime<Utc>>().unwrap(),
        )
        .await
        .unwrap()
    }

    /// Logs a scan at `at`; `None` is an anonymous network scan.
    async fn scanned(&self, username: Option<&str>, pages: u32, at: &str) -> i64 {
        let user_id = match username {
            Some(username) => Some(self.user_id(username).await),
            None => None,
        };
        scans::record(
            &self.database,
            NewScan {
                user_id,
                username: username.unwrap_or_default().into(),
                scanner: "mock:0".into(),
                result_id: None,
                source: if user_id.is_some() {
                    Source::Web
                } else {
                    Source::Escl
                },
                format: "pdf".into(),
                pages,
            },
            at.parse::<DateTime<Utc>>().unwrap(),
        )
        .await
        .unwrap()
    }

    /// Sends a GET as the user and returns the status and the body.
    async fn get(&self, uri: &str, cookie: &str) -> (StatusCode, String) {
        let request = Request::get(uri)
            .header(header::ACCEPT, "text/html")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let response = self.send(request).await;
        (response.status(), text(response).await)
    }

    async fn scan_rows(&self) -> Vec<scan_jobs::Model> {
        scan_jobs::Entity::find().all(&self.database).await.unwrap()
    }

    /// Scans a PDF on the mock scanner as the user; returns the outcome.
    async fn scan_pdf(&self, cookie: &str) -> serde_json::Value {
        let body = serde_json::json!({
            "id": "mock:0",
            "settings": { "resolution": 150, "mode": "Gray", "source": "Flatbed", "area": null },
            "selection": null,
            "format": "Pdf",
        });
        let response = self
            .call("scan", cookie, "application/json", body.to_string())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_str(&text(response).await).unwrap()
    }

    async fn save_scan(&self, cookie: &str, id: &str) -> Response<Body> {
        std::fs::create_dir_all(self._dir.path().join("saved")).unwrap();
        let request = Request::post(format!("/scan/results/{id}/save"))
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, ORIGIN)
            .header("sec-fetch-site", "same-origin")
            .body(Body::empty())
            .unwrap();
        self.send(request).await
    }
}

#[tokio::test]
async fn a_scan_is_logged_and_a_preview_is_not() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let alice = harness.sign_in("alice-subject", "alice").await;
    let response = harness
        .call(
            "preview",
            &alice,
            "application/x-www-form-urlencoded",
            "id=mock%3A0".into(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(harness.scan_rows().await.is_empty());

    let outcome = harness.scan_pdf(&alice).await;
    let rows = harness.scan_rows().await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    let user = users::find_by_username(&harness.database, "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.user_id, Some(user.id));
    assert_eq!(row.username, "alice");
    assert_eq!(row.scanner, "mock:0");
    assert_eq!(row.format, "pdf");
    assert_eq!(row.source, "web");
    assert_eq!(row.pages, outcome["pages"].as_i64().unwrap());
    assert_eq!(row.result_id.as_deref(), outcome["id"].as_str());
    assert!(!row.saved);
}

#[tokio::test]
async fn saving_marks_the_scan_saved() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let alice = harness.sign_in("alice-subject", "alice").await;
    let outcome = harness.scan_pdf(&alice).await;
    let response = harness
        .save_scan(&alice, outcome["id"].as_str().unwrap())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(harness.scan_rows().await[0].saved);
}

#[tokio::test]
async fn scanning_works_when_the_log_cannot_be_written() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let alice = harness.sign_in("alice-subject", "alice").await;
    harness
        .database
        .execute_unprepared("DROP TABLE scan_jobs")
        .await
        .unwrap();
    let outcome = harness.scan_pdf(&alice).await;
    assert!(outcome["file_name"].as_str().unwrap().ends_with(".pdf"));
    let response = harness
        .save_scan(&alice, outcome["id"].as_str().unwrap())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// The row of the table that names `name`.
fn row<'a>(html: &'a str, name: &str) -> &'a str {
    html.split("<tr")
        .find(|row| row.contains(&format!(">{name}<")))
        .unwrap_or_else(|| panic!("no row for {name}: {html}"))
}

/// The cells of a row, as text.
fn cells(row: &str) -> Vec<String> {
    row.split("<td")
        .skip(1)
        .map(|cell| {
            let start = cell.find('>').unwrap() + 1;
            let end = cell.find("</td>").unwrap();
            cell[start..end].replace("<!>", "").trim().to_owned()
        })
        .collect()
}

const NOVEMBER: &str = "/administration/usage?from=2026-11-01&to=2026-11-30";

#[tokio::test]
async fn the_usage_page_totals_the_range() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let admin = administrator(&harness, "admin-subject", "admin").await;
    harness
        .sign_in_in("alice-subject", "alice", &["design"])
        .await;
    harness.sign_in("bob-subject", "bob").await;
    let november = "2026-11-17T10:00:00Z";
    harness
        .printed("alice", "office", "a.pdf", Some(3), false, november)
        .await;
    harness
        .printed("alice", "lab", "b.pdf", Some(2), true, november)
        .await;
    harness
        .printed("bob", "office", "c.pdf", Some(1), false, november)
        .await;
    harness
        .printed(
            "bob",
            "office",
            "d.pdf",
            Some(40),
            false,
            "2026-12-01T00:00:00Z",
        )
        .await;
    harness.scanned(Some("alice"), 4, november).await;
    harness.scanned(None, 2, november).await;

    let html = harness.page(NOVEMBER, &admin).await;
    assert!(html.contains(">Total<"), "{html}");
    assert!(html.contains(r#"value="2026-11-01""#), "{html}");
    assert!(html.contains(r#"value="2026-11-30""#), "{html}");
    // Jobs, pages, sheets, color, black and white, cost, unknown; scans, pages, saved.
    assert_eq!(
        cells(row(&html, "alice")),
        ["2", "5", "5", "2", "3", "0.81 EUR", "0", "1", "4", "0"]
    );
    assert_eq!(
        cells(row(&html, "bob")),
        ["1", "1", "1", "0", "1", "0.05 EUR", "0", "0", "0", "0"]
    );
    assert_eq!(
        cells(row(&html, "Anonymous (network)")),
        ["0", "0", "0", "0", "0", "0.00 EUR", "0", "1", "2", "0"]
    );
    assert!(html.contains("0.86 EUR"), "{html}");
    // The tables keep the range.
    for by in ["user", "group", "printer", "scanner"] {
        let link = format!(
            r#"href="/administration/usage?from=2026-11-01&amp;to=2026-11-30&amp;by={by}""#
        );
        assert!(html.contains(&link), "{by}: {html}");
    }

    let html = harness
        .page(&format!("{NOVEMBER}&by=printer"), &admin)
        .await;
    assert_eq!(
        cells(row(&html, "office")),
        ["2", "4", "4", "0", "4", "0.20 EUR", "0"]
    );
    assert!(!html.contains(">alice<"), "{html}");
    let html = harness.page(&format!("{NOVEMBER}&by=group"), &admin).await;
    assert_eq!(cells(row(&html, "design"))[1], "5");
    let html = harness
        .page(&format!("{NOVEMBER}&by=scanner"), &admin)
        .await;
    assert_eq!(cells(row(&html, "mock:0")), ["2", "6", "0"]);
    // An unknown table is the user table.
    let html = harness.page(&format!("{NOVEMBER}&by=bogus"), &admin).await;
    assert!(html.contains(">alice<"), "{html}");
}

#[tokio::test]
async fn the_usage_page_defaults_to_this_month() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let admin = administrator(&harness, "admin-subject", "admin").await;
    let today = Utc::now().date_naive();
    let first = today.with_day(1).unwrap();
    let last = (first + Months::new(1)).pred_opt().unwrap();
    let html = harness.page("/administration/usage", &admin).await;
    assert!(html.contains(&format!(r#"value="{first}""#)), "{html}");
    assert!(html.contains(&format!(r#"value="{last}""#)), "{html}");
}

#[tokio::test]
async fn invalid_dates_are_refused() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let admin = administrator(&harness, "admin-subject", "admin").await;
    for query in [
        "from=2026-13-01&to=2026-12-01",
        "from=2026-11-02&to=2026-11-01",
        "from=2026-11-01",
        "to=2026-11-01",
        "from=1999-12-31&to=2026-01-01",
        "from=2026-01-01&to=3000-01-01",
        "from=9999-12-31&to=9999-12-31",
        "from=%FF&to=x",
    ] {
        let (status, html) = harness
            .get(&format!("/administration/usage?{query}"), &admin)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        assert!(
            html.contains("Enter two dates between 2000 and 2999"),
            "{query}: {html}"
        );
        assert!(!html.contains("<table"), "{query}: {html}");
        assert!(!html.contains(">Total<"), "{query}: {html}");
    }
}

#[tokio::test]
async fn only_administrators_see_usage() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let admin = administrator(&harness, "admin-subject", "admin").await;
    let alice = harness.sign_in("alice-subject", "alice").await;
    harness
        .printed(
            "alice",
            "office",
            "secret.pdf",
            Some(3),
            false,
            "2026-11-17T10:00:00Z",
        )
        .await;
    let (status, html) = harness.get(NOVEMBER, &alice).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(html.contains("Page not found"), "{html}");
    assert!(!html.contains(">Total<"), "{html}");
    assert!(!html.contains(">Usage<"), "{html}");

    let html = harness.page("/administration/users", &admin).await;
    assert!(html.contains(r#"href="/administration/usage""#), "{html}");
}

#[tokio::test]
async fn an_empty_range_says_so() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let admin = administrator(&harness, "admin-subject", "admin").await;
    let html = harness.page(NOVEMBER, &admin).await;
    assert!(
        html.contains("Nothing was printed or scanned in this range."),
        "{html}"
    );
    assert!(!html.contains("<table"), "{html}");
}
