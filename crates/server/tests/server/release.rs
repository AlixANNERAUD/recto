//! Held jobs on the Jobs page: released through the print path, its access rules, quotas and
//! ledger, at most once; deleted; and their documents shown to their owner alone.

use chrono::{TimeDelta, Utc};
use recto::serve::{poll_once, sweep_held_once};
use recto_domain::{
    held::{self, HeldState, NewHeld, Stored},
    ledger,
    quotas::{self, Enforcement, Metric, NewRule, Period, Subject},
};
use recto_printing::testing::sample_pdf;

use super::*;

/// Holds `document` for the user, as a device would have sent it, asking for `requested`.
async fn hold_with(
    harness: &Harness,
    user: i64,
    document: &[u8],
    requested: &str,
) -> (i64, String) {
    let file = harness.held.write(document).unwrap();
    let id = held::receive(
        &harness.database,
        NewHeld {
            user_id: user,
            device_password_id: None,
            name: "minutes.pdf".into(),
            requested: requested.into(),
            stored: Stored {
                file: file.clone(),
                stored_format: "application/pdf".into(),
                received_format: Some("application/pdf".into()),
                document_pages: Some(2),
                size: document.len() as i64,
            },
            now: Utc::now(),
            lifetime: TimeDelta::hours(24),
        },
        i64::MAX,
    )
    .await
    .unwrap();
    (id, file)
}

async fn hold(harness: &Harness, user: i64) -> (i64, String) {
    hold_with(harness, user, &sample_pdf(2), "{}").await
}

/// Alice, signed in with her password.
async fn alice(harness: &Harness) -> (i64, String) {
    let id = harness.local_user("alice", ALICE_PASSWORD).await;
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    (id, cookie)
}

fn release(id: i64, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    form(&format!("/jobs/held/{id}/release"), Some(cookie), fields)
}

async fn job(harness: &Harness, user: i64, id: i64) -> Option<held::HeldJob> {
    held::get(&harness.database, user, id).await.unwrap()
}

async fn ledger_rows(harness: &Harness) -> Vec<print_jobs::Model> {
    print_jobs::Entity::find()
        .all(&harness.database)
        .await
        .unwrap()
}

#[tokio::test]
async fn releasing_prints_through_quotas_and_the_ledger() {
    let harness = Harness::new().await;
    let (alice, cookie) = alice(&harness).await;
    let document = sample_pdf(2);
    let (id, file) = hold_with(&harness, alice, &document, "{}").await;

    let response = harness
        .send(release(
            id,
            &cookie,
            &[
                ("printer", "office"),
                ("copies", "2"),
                ("color_mode", "monochrome"),
            ],
        ))
        .await;
    assert_redirect(&response, "/jobs?submitted=8");

    let submitted = harness.print.submitted();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].printer, "office");
    assert_eq!(submitted[0].user, "alice");
    assert_eq!(submitted[0].job_name, "minutes.pdf");
    assert_eq!(submitted[0].options.copies, 2);
    assert_eq!(&submitted[0].document[..], &document[..]);
    let rows = ledger_rows(&harness).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pages, Some(4));
    assert_eq!(rows[0].cups_job_id, Some(8));
    let released = job(&harness, alice, id).await.unwrap();
    assert_eq!(released.state, HeldState::Released);
    assert_eq!(released.ledger_entry_id, Some(rows[0].id));
    // Printed: the document goes at once, not with the row a day later.
    assert!(harness.held.read(&file).is_err());
    // A second release finds nothing to print.
    let response = harness
        .send(release(id, &cookie, &[("printer", "office")]))
        .await;
    assert_redirect(&response, "/jobs?error=held_gone");
    assert_eq!(harness.print.submitted().len(), 1);
}

#[tokio::test]
async fn released_and_counted_in_one_transaction() {
    let harness = Harness::new().await;
    let (alice, cookie) = alice(&harness).await;
    let (id, _) = hold(&harness, alice).await;
    // CUPS may have queued the job and never said so, as when recto stops right after it was
    // counted.
    harness.print.lose_answers("timed out");
    let response = harness
        .send(release(id, &cookie, &[("printer", "office")]))
        .await;
    assert_redirect(&response, "/jobs?error=unconfirmed");
    let rows = ledger_rows(&harness).await;
    assert_eq!(rows.len(), 1);
    let released = job(&harness, alice, id).await.unwrap();
    assert_eq!(released.state, HeldState::Released);
    assert_eq!(released.ledger_entry_id, Some(rows[0].id));

    // A job with a ledger entry never goes back to be printed again.
    let later = Utc::now() + held::CLAIM_LIFETIME + TimeDelta::minutes(1);
    sweep_held_once(&harness.database, &harness.held, later).await;
    assert_eq!(
        job(&harness, alice, id).await.unwrap().state,
        HeldState::Released
    );
    // The entry CUPS never confirmed is refunded once its grace is over.
    let refunded = ledger::refund_unattached(
        &harness.database,
        Utc::now() + ledger::ATTACH_GRACE + TimeDelta::minutes(1),
    )
    .await
    .unwrap();
    assert_eq!(refunded, 1);
}

#[tokio::test]
async fn a_refused_release_stays_held() {
    let harness = Harness::new().await;
    let (alice, cookie) = alice(&harness).await;
    let (id, file) = hold(&harness, alice).await;
    let rule = quotas::create(
        &harness.database,
        NewRule {
            name: "Monthly pages".into(),
            enabled: true,
            subject: Subject::Everyone,
            printers: vec![],
            metric: Metric::Pages,
            period: Period::Month,
            limit: 1,
            enforcement: Enforcement::Hard,
        },
    )
    .await
    .unwrap();

    let response = harness
        .send(release(id, &cookie, &[("printer", "office")]))
        .await;
    assert_redirect(&response, &format!("/jobs?error=quota&rule={rule}"));
    assert!(harness.print.submitted().is_empty());
    assert!(ledger_rows(&harness).await.is_empty());
    let refused = job(&harness, alice, id).await.unwrap();
    assert_eq!(refused.state, HeldState::Held);
    assert_eq!(refused.refusal.as_deref(), Some("quota"));
    assert!(harness.held.read(&file).is_ok());
    // The Jobs page names the rule and what is left, as the Print page does.
    let response = harness
        .send(page(
            &format!("/jobs?error=quota&rule={rule}"),
            Some(&cookie),
        ))
        .await;
    let html = text(response).await;
    assert!(html.contains("Monthly pages"), "{html}");

    // A printer alice may not use is one that does not exist.
    quotas::delete(&harness.database, rule).await.unwrap();
    harness.member_of("sam-subject", "sam", &["staff"]).await;
    restrict_to_staff(&harness, DeviceKind::Printer, "office").await;
    let response = harness
        .send(release(id, &cookie, &[("printer", "office")]))
        .await;
    assert_redirect(&response, "/jobs?error=printer_not_found");
    assert_eq!(
        job(&harness, alice, id).await.unwrap().state,
        HeldState::Held
    );
}

#[tokio::test]
async fn a_cups_refusal_returns_the_job() {
    let harness = Harness::new().await;
    let (alice, cookie) = alice(&harness).await;
    let (id, file) = hold(&harness, alice).await;
    harness.print.refuse_submissions("printer on fire");
    let response = harness
        .send(release(id, &cookie, &[("printer", "office")]))
        .await;
    assert_redirect(&response, "/jobs?error=refused");
    let rows = ledger_rows(&harness).await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].refunded);
    let returned = job(&harness, alice, id).await.unwrap();
    assert_eq!(returned.state, HeldState::Held);
    assert_eq!(returned.ledger_entry_id, None);
    assert_eq!(returned.refusal.as_deref(), Some("refused"));
    assert!(harness.held.read(&file).is_ok());
    // The page says why, and it can be released again.
    let html = text(harness.send(page("/jobs", Some(&cookie))).await).await;
    assert!(
        html.contains("The printing service refused the request."),
        "{html}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_releases_print_once() {
    let harness = Harness::new().await;
    let (alice, cookie) = alice(&harness).await;
    let (id, _) = hold(&harness, alice).await;
    let (first, second) = tokio::join!(
        harness.send(release(id, &cookie, &[("printer", "office")])),
        harness.send(release(id, &cookie, &[("printer", "office")]))
    );
    let mut locations = vec![location(&first), location(&second)];
    locations.sort();
    assert_eq!(locations, ["/jobs?error=held_gone", "/jobs?submitted=8"]);
    assert_eq!(harness.print.submitted().len(), 1);
    assert_eq!(ledger_rows(&harness).await.len(), 1);
    // The poller finds nothing wrong with it.
    poll_once(&*harness.print, &harness.database).await;
    assert!(!ledger_rows(&harness).await[0].refunded);
}

#[tokio::test]
async fn without_a_database_a_held_job_does_not_print() {
    let harness = Harness::new().await;
    let (alice, cookie) = alice(&harness).await;
    let (id, _) = hold(&harness, alice).await;
    // No quota rule: the print form would print without a ledger, a held job must not.
    harness.drop_table("print_jobs").await;
    let response = harness
        .send(release(id, &cookie, &[("printer", "office")]))
        .await;
    assert_redirect(&response, "/jobs?error=unavailable");
    assert!(harness.print.submitted().is_empty());
    assert_eq!(
        job(&harness, alice, id).await.unwrap().state,
        HeldState::Held
    );
}

#[tokio::test]
async fn held_documents_are_the_owners_only() {
    let harness = Harness::new().await;
    let (alice, cookie) = alice(&harness).await;
    let document = sample_pdf(2);
    let (id, file) = hold_with(&harness, alice, &document, "{}").await;
    let bob = harness.sign_in("bob-subject", "bob").await;
    let admin = harness.administrator("root-subject", "root").await;

    for other in [&bob, &admin] {
        let response = harness
            .send(page(&format!("/jobs/held/{id}/document"), Some(other)))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        for action in ["release", "delete"] {
            let response = harness
                .send(form(
                    &format!("/jobs/held/{id}/{action}"),
                    Some(other),
                    &[("printer", "office")],
                ))
                .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{action}");
        }
        // Not on their Jobs page either.
        let html = text(harness.send(page("/jobs", Some(other))).await).await;
        assert!(!html.contains(&format!("/jobs/held/{id}/")), "{html}");
    }
    assert!(harness.print.submitted().is_empty());
    assert_eq!(
        job(&harness, alice, id).await.unwrap().state,
        HeldState::Held
    );

    let response = harness
        .send(page(&format!("/jobs/held/{id}/document"), Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(headers[header::CONTENT_TYPE], "application/pdf");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert!(
        headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("inline")
    );
    // Chrome's PDF viewer does not run in a sandbox.
    let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(!policy.contains("sandbox"), "{policy}");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], &document[..]);

    // Cross-site posts are refused like every other form.
    let mut request = release(id, &cookie, &[("printer", "office")]);
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    request
        .headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());
    assert_eq!(harness.send(request).await.status(), StatusCode::FORBIDDEN);

    // Deleting takes row and document.
    let response = harness
        .send(form(&format!("/jobs/held/{id}/delete"), Some(&cookie), &[]))
        .await;
    assert_redirect(&response, "/jobs?done=deleted");
    assert!(job(&harness, alice, id).await.is_none());
    assert!(harness.held.read(&file).is_err());
    let html = text(
        harness
            .send(page("/jobs?done=deleted", Some(&cookie)))
            .await,
    )
    .await;
    assert!(html.contains("The document was deleted."), "{html}");
}

#[tokio::test]
async fn the_jobs_page_shows_what_waits() {
    let harness = Harness::new().await;
    let (alice, cookie) = alice(&harness).await;
    // Nothing waits: no section.
    let html = text(harness.send(page("/jobs", Some(&cookie))).await).await;
    assert!(!html.contains("Waiting for you"), "{html}");

    let phone = recto_domain::device_passwords::create(
        &harness.database,
        alice,
        "Alice's phone",
        Utc::now(),
    )
    .await
    .unwrap();
    let file = harness.held.write(&sample_pdf(3)).unwrap();
    let id = held::receive(
        &harness.database,
        NewHeld {
            user_id: alice,
            device_password_id: Some(phone.id),
            name: "boarding pass.pdf".into(),
            requested: serde_json::to_string(&recto_printing::printer::Requested {
                copies: Some(3),
                sides: Some(Sides::TwoSidedLongEdge),
                color_mode: Some(ColorMode::Monochrome),
                page_ranges: Some(PageRanges(vec![(1, 2), (5, 5)])),
                media: None,
            })
            .unwrap(),
            stored: Stored {
                file,
                stored_format: "application/pdf".into(),
                received_format: Some("application/pdf".into()),
                document_pages: Some(3),
                size: 100,
            },
            now: Utc::now(),
            lifetime: TimeDelta::hours(24),
        },
        i64::MAX,
    )
    .await
    .unwrap();
    let html = text(harness.send(page("/jobs", Some(&cookie))).await).await;
    assert!(html.contains("Waiting for you"), "{html}");
    assert!(html.contains("boarding pass.pdf"), "{html}");
    assert!(
        html.contains("Alice&#x27;s phone") || html.contains("Alice's phone"),
        "{html}"
    );
    assert!(
        html.contains(&format!("href=\"/jobs/held/{id}/document\"")),
        "{html}"
    );
    assert!(
        html.contains(&format!("action=\"/jobs/held/{id}/release\"")),
        "{html}"
    );
    assert!(
        html.contains(&format!("action=\"/jobs/held/{id}/delete\"")),
        "{html}"
    );
    // The form starts from what the device asked for.
    assert!(html.contains("value=\"3\""), "{html}");
    assert!(html.contains("value=\"1-2, 5\""), "{html}");
    assert!(html.contains("<option value=\"office\" selected"), "{html}");
    assert!(
        html.contains("<option value=\"two-sided-long-edge\" selected"),
        "{html}"
    );
    assert!(
        html.contains("<option value=\"monochrome\" selected"),
        "{html}"
    );
}
