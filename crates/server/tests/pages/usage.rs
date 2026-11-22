//! Usage: the scan log, the Usage administration page and its CSV exports.

use recto_entity::scan_jobs;
use sea_orm::ConnectionTrait;

use super::*;

impl Harness {
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
