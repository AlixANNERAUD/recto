//! Print quotas: enforced when printing, shown on the Print and Jobs pages, managed on the
//! Quotas and Users administration pages.

use chrono::Utc;
use recto_domain::{
    ledger::{self, NewEntry},
    quotas::{self, Enforcement, Metric, NewRule, Period, Subject},
};
use sea_orm::ConnectionTrait;

use super::*;

/// "Monthly pages": everyone, `limit` pages a month on every printer.
fn monthly_pages(limit: i64, enforcement: Enforcement) -> NewRule {
    NewRule {
        name: "Monthly pages".into(),
        enabled: true,
        subject: Subject::Everyone,
        printers: vec![],
        metric: Metric::Pages,
        period: Period::Month,
        limit,
        enforcement,
    }
}

impl Harness {
    async fn rule(&self, rule: NewRule) -> i64 {
        quotas::create(&self.database, rule).await.unwrap()
    }

    /// Enters a job of `pages` black-and-white pages for the user, as printed just now.
    async fn used(&self, username: &str, pages: u32) {
        let user_id = self.user_id(username).await;
        ledger::insert(
            &self.database,
            NewEntry {
                user_id,
                username: username.into(),
                printer: "office".into(),
                document_name: "earlier.pdf".into(),
                document_format: "application/pdf".into(),
                document_pages: Some(pages),
                copies: 1,
                sides: "one-sided".into(),
                color_mode: "monochrome".into(),
                counts: ledger::counts(Some(pages), 1, false, false, Prices::default()),
            },
            Utc::now(),
        )
        .await
        .unwrap();
    }

    async fn ledger_rows(&self) -> Vec<print_jobs::Model> {
        print_jobs::Entity::find()
            .all(&self.database)
            .await
            .unwrap()
    }

    async fn html(&self, uri: &str, cookie: &str) -> String {
        let response = self.send(page(uri, Some(cookie))).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        text(response).await
    }

    /// Prints a black-and-white PDF of `pages` pages on the office printer.
    async fn print_pages(&self, cookie: &str, pages: u32) -> Response<Body> {
        let pdf = recto_printing::testing::sample_pdf(pages);
        print(
            self,
            cookie,
            &[("printer", "office"), ("color_mode", "monochrome")],
            Some(("report.pdf", &pdf)),
        )
        .await
    }
}

#[tokio::test]
async fn a_hard_rule_refuses_and_names_what_is_left() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let rule = harness.rule(monthly_pages(5, Enforcement::Hard)).await;
    harness.used("alice", 4).await;

    let response = harness.print_pages(&cookie, 2).await;
    assert_redirect(&response, &format!("/?error=quota&rule={rule}"));
    assert!(harness.print.submitted().is_empty());
    assert_eq!(harness.ledger_rows().await.len(), 1);

    let html = harness
        .html(&format!("/?error=quota&rule={rule}"), &cookie)
        .await;
    assert!(
        html.contains("This job needs more than you have left of “Monthly pages”: 1 of 5 pages left this month."),
        "{html}"
    );
    // What fits still prints.
    let response = harness.print_pages(&cookie, 1).await;
    assert_redirect(&response, "/jobs?submitted=8");
}

#[tokio::test]
async fn racing_jobs_cannot_both_pass() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let rule = harness.rule(monthly_pages(1, Enforcement::Hard)).await;

    let (first, second) = tokio::join!(
        harness.print_pages(&cookie, 1),
        harness.print_pages(&cookie, 1)
    );
    assert_eq!(harness.print.submitted().len(), 1);
    let mut locations = [location(&first), location(&second)];
    locations.sort();
    assert_eq!(
        locations,
        [
            format!("/?error=quota&rule={rule}"),
            "/jobs?submitted=8".to_owned()
        ]
    );
    assert_eq!(harness.ledger_rows().await.len(), 1);
}

#[tokio::test]
async fn a_soft_rule_prints_and_warns() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let rule = harness.rule(monthly_pages(1, Enforcement::Soft)).await;

    let response = harness.print_pages(&cookie, 2).await;
    let target = format!("/jobs?submitted=8&over={rule}");
    assert_redirect(&response, &target);
    assert_eq!(harness.print.submitted().len(), 1);

    let html = harness.html(&target, &cookie).await;
    assert!(
        html.contains("This job went over “Monthly pages”: 0 of 1 page left this month."),
        "{html}"
    );
    // A crafted list names nothing it does not know.
    let html = harness.html("/jobs?submitted=8&over=999", &cookie).await;
    assert!(html.contains("This job went over a print quota."), "{html}");
}

#[tokio::test]
async fn cups_refusal_refunds_the_counted_job() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    harness.rule(monthly_pages(10, Enforcement::Hard)).await;
    harness
        .print
        .refuse_submissions("client-error-not-possible");

    let response = harness.print_pages(&cookie, 2).await;
    assert_redirect(&response, "/?error=refused");
    let rows = harness.ledger_rows().await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].refunded);
    let html = harness.html("/?error=refused", &cookie).await;
    assert!(html.contains("The printing service refused the request."));
    assert!(html.contains("10 of 10 pages left this month"), "{html}");
}

#[tokio::test]
async fn an_uncountable_pdf_is_refused_only_when_pages_count() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let pages = harness.rule(monthly_pages(100, Enforcement::Hard)).await;
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("report.pdf", PDF)),
    )
    .await;
    assert_redirect(&response, "/?error=uncountable");
    assert!(harness.print.submitted().is_empty());
    let html = harness.html("/?error=uncountable", &cookie).await;
    assert!(
        html.contains("could not count the pages of this PDF"),
        "{html}"
    );

    quotas::delete(&harness.database, pages).await.unwrap();
    harness
        .rule(NewRule {
            name: "Jobs".into(),
            metric: Metric::Jobs,
            ..monthly_pages(100, Enforcement::Hard)
        })
        .await;
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("report.pdf", PDF)),
    )
    .await;
    assert_redirect(&response, "/jobs?submitted=8");
    let rows = harness.ledger_rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pages, None);
}

#[tokio::test]
async fn quotas_fail_closed() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    harness.rule(monthly_pages(100, Enforcement::Hard)).await;
    for table in ["quota_adjustments", "quota_rule_printers", "quota_rules"] {
        harness
            .database
            .execute_unprepared(&format!("DROP TABLE {table}"))
            .await
            .unwrap();
    }
    let response = harness.print_pages(&cookie, 1).await;
    assert_redirect(&response, "/?error=unavailable");
    assert!(harness.print.submitted().is_empty());

    // Without rules, the ledger is best effort.
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    harness
        .database
        .execute_unprepared("DROP TABLE print_jobs")
        .await
        .unwrap();
    let response = harness.print_pages(&cookie, 1).await;
    assert_redirect(&response, "/jobs?submitted=8");
    assert_eq!(harness.print.submitted().len(), 1);
}

#[tokio::test]
async fn the_print_page_shows_what_is_left() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    harness.rule(monthly_pages(100, Enforcement::Hard)).await;
    harness
        .rule(NewRule {
            name: "Office color".into(),
            printers: vec!["office".into()],
            metric: Metric::ColorPages,
            period: Period::Rolling(7),
            ..monthly_pages(20, Enforcement::Soft)
        })
        .await;
    // On a printer alice cannot reach, a rule says nothing to her.
    harness
        .rule(NewRule {
            name: "Lab".into(),
            printers: vec!["lab".into()],
            ..monthly_pages(5, Enforcement::Hard)
        })
        .await;
    harness.used("alice", 30).await;

    let html = harness.html("/", &cookie).await;
    assert!(html.contains("What you have left"), "{html}");
    assert!(html.contains("70 of 100 pages left this month"), "{html}");
    assert!(
        html.contains("20 of 20 color pages left in the last 7 days, on office"),
        "{html}"
    );
    assert!(!html.contains(">Lab<"), "{html}");
}

#[tokio::test]
async fn job_costs_show_to_owner_and_administrators_only() {
    let harness = Harness::new().await;
    prices::set(&harness.database, "office", euros(5, 20, 1))
        .await
        .unwrap();
    let alice = harness.sign_in("alice-subject", "alice").await;
    let bob = harness.sign_in("bob-subject", "bob").await;
    let admin = harness.administrator("admin-subject", "admin").await;
    let response = harness.print_pages(&alice, 2).await;
    assert_redirect(&response, "/jobs?submitted=8");

    // 2 black-and-white pages on 2 sheets.
    for cookie in [&alice, &admin] {
        let html = harness.html("/jobs", cookie).await;
        assert!(html.contains("2 pages"), "{html}");
        assert!(html.contains("0.12 EUR"), "{html}");
    }
    let html = harness.html("/jobs", &bob).await;
    assert!(html.contains("report.pdf"), "{html}");
    assert!(
        !html.contains("0.12 EUR") && !html.contains("2 pages"),
        "{html}"
    );
}

#[tokio::test]
async fn nothing_changes_without_rules() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.html("/", &cookie).await;
    assert!(!html.contains("What you have left"), "{html}");
    let response = harness.print_pages(&cookie, 2).await;
    assert_redirect(&response, "/jobs?submitted=8");

    // A disabled rule is no rule.
    harness
        .rule(NewRule {
            enabled: false,
            ..monthly_pages(1, Enforcement::Hard)
        })
        .await;
    let html = harness.html("/", &cookie).await;
    assert!(!html.contains("What you have left"), "{html}");
    let response = harness.print_pages(&cookie, 2).await;
    assert_redirect(&response, "/jobs?submitted=9");
}

impl Harness {
    async fn group_id(&self, name: &str) -> i64 {
        groups::Entity::find()
            .filter(groups::Column::Name.eq(name))
            .one(&self.database)
            .await
            .unwrap()
            .expect("the group exists")
            .id
    }
}

/// A rule form with these fields over the defaults: everyone, 100 pages a month, hard, enabled.
fn rule_form<'a>(fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut form: Vec<(&str, &str)> = vec![
        ("name", "Monthly pages"),
        ("subject", "everyone"),
        ("metric", "pages"),
        ("limit", "100"),
        ("period", "month"),
        ("rolling_days", ""),
        ("enforcement", "hard"),
        ("enabled", "true"),
    ];
    form.retain(|(key, _)| !fields.iter().any(|(name, _)| name == key));
    form.extend_from_slice(fields);
    form
}

#[tokio::test]
async fn an_administrator_creates_rules_for_each_subject_and_period() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    harness
        .member_of("alice-subject", "alice", &["design"])
        .await;
    let design = harness.group_id("design").await;
    let alice = harness.user_id("alice").await;
    let group = format!("group:{design}");
    let user = format!("user:{alice}");
    let pool = format!("pool:{design}");

    let forms = [
        rule_form(&[]),
        rule_form(&[
            ("name", "Design colour"),
            ("subject", &group),
            ("metric", "color_pages"),
            ("period", "week"),
            ("enforcement", "soft"),
        ]),
        rule_form(&[
            ("name", "Alice"),
            ("subject", &user),
            ("period", "rolling"),
            ("rolling_days", "7"),
            ("printers", "office"),
            ("printers", "Lab"),
        ]),
        rule_form(&[
            ("name", "Design budget"),
            ("subject", &pool),
            ("metric", "cost"),
            ("limit", "2,50"),
            ("period", "total"),
        ]),
        rule_form(&[
            ("name", "Jobs a day"),
            ("metric", "jobs"),
            ("period", "day"),
        ]),
        rule_form(&[
            ("name", "Sheets"),
            ("metric", "sheets"),
            ("period", "quarter"),
        ]),
        rule_form(&[
            ("name", "Mono"),
            ("metric", "mono_pages"),
            ("period", "year"),
        ]),
    ];
    for fields in &forms {
        let response = harness
            .send(form("/administration/quotas", Some(&admin), fields))
            .await;
        assert_redirect(&response, "/administration/quotas?done=created");
    }
    let rules = quotas::rules(&harness.database).await.unwrap();
    assert_eq!(rules.len(), 7);
    assert_eq!(rules[0].subject, Subject::Everyone);
    assert_eq!(rules[0].limit, 100);
    assert_eq!(rules[1].subject, Subject::Group(design));
    assert_eq!(rules[1].metric, Metric::ColorPages);
    assert_eq!(rules[1].enforcement, Enforcement::Soft);
    assert_eq!(rules[2].subject, Subject::User(alice));
    assert_eq!(rules[2].period, Period::Rolling(7));
    assert_eq!(rules[2].printers, ["Lab", "office"]);
    assert_eq!(rules[3].subject, Subject::Pool(design));
    assert_eq!(rules[3].metric, Metric::Cost);
    assert_eq!(rules[3].limit, 250);
    assert_eq!(rules[3].period, Period::Total);

    let html = harness
        .html("/administration/quotas?done=created", &admin)
        .await;
    assert!(html.contains("The rule was created."), "{html}");
    assert!(html.contains("Quotas · Administration"), "{html}");
    for text in [
        "100 pages a month, on all printers, hard",
        "Each member of design",
        "100 color pages a week, on all printers, soft",
        "100 pages in any 7 days, on Lab, office, hard",
        "design together",
        "2.50 EUR in total, on all printers, hard",
        "100 jobs a day",
        "value=\"2.50\"",
    ] {
        assert!(html.contains(text), "{text}: {html}");
    }
}

#[tokio::test]
async fn invalid_rules_are_refused_with_the_draft_kept() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    for (fields, message) in [
        (vec![("name", " ")], "The rule needs a name."),
        (vec![("limit", "-1")], "The limit cannot be negative."),
        (vec![("limit", "many")], "The limit must be a whole number"),
        (
            vec![("metric", "cost"), ("limit", "0.001")],
            "The limit must be a whole number",
        ),
        (
            vec![("period", "rolling"), ("rolling_days", "")],
            "A rolling period lasts 1 to 366 days.",
        ),
        (
            vec![("subject", "group:999")],
            "That group or user does not exist.",
        ),
        (vec![("metric", "ink")], "The form was incomplete."),
    ] {
        let response = harness
            .send(form(
                "/administration/quotas",
                Some(&admin),
                &rule_form(&fields),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{fields:?}");
        let html = text(response).await;
        assert!(html.contains(message), "{message}: {html}");
        let limit = fields
            .iter()
            .find(|(key, _)| *key == "limit")
            .map_or("100", |(_, value)| value);
        assert!(html.contains(&format!("value=\"{limit}\"")), "{html}");
    }
    assert!(quotas::rules(&harness.database).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_administrator_edits_disables_and_deletes_a_rule() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    let alice = harness.sign_in("alice-subject", "alice").await;
    let rule = harness.rule(monthly_pages(1, Enforcement::Hard)).await;
    let path = format!("/administration/quotas/{rule}");

    let response = harness
        .send(form(
            &path,
            Some(&admin),
            &rule_form(&[("name", "Renamed"), ("limit", "1")]),
        ))
        .await;
    assert_redirect(&response, "/administration/quotas?done=saved");
    assert_eq!(
        quotas::rule(&harness.database, rule).await.unwrap().name,
        "Renamed"
    );
    // A refused change keeps what was typed in that rule's form.
    let response = harness
        .send(form(
            &path,
            Some(&admin),
            &rule_form(&[("name", "Typed"), ("limit", "-3")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = text(response).await;
    assert!(
        html.contains("value=\"Typed\"") && html.contains("value=\"-3\""),
        "{html}"
    );

    let response = harness.print_pages(&alice, 2).await;
    assert_redirect(&response, &format!("/?error=quota&rule={rule}"));
    let response = harness
        .send(form(
            &format!("{path}/enabled"),
            Some(&admin),
            &[("enabled", "false")],
        ))
        .await;
    assert_redirect(&response, "/administration/quotas?done=disabled");
    let response = harness.print_pages(&alice, 2).await;
    assert_redirect(&response, "/jobs?submitted=8");
    let html = harness.html("/administration/quotas", &admin).await;
    assert!(html.contains("Disabled"), "{html}");

    let response = harness
        .send(form(&format!("{path}/delete"), Some(&admin), &[]))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(text(response).await.contains("Tick the box"));
    assert!(quotas::rule(&harness.database, rule).await.is_ok());
    let response = harness
        .send(form(
            &format!("{path}/delete"),
            Some(&admin),
            &[("confirm", "yes")],
        ))
        .await;
    assert_redirect(&response, "/administration/quotas?done=deleted");
    assert!(quotas::rules(&harness.database).await.unwrap().is_empty());
    let response = harness
        .send(form(
            &format!("{path}/delete"),
            Some(&admin),
            &[("confirm", "yes")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn quotas_need_an_administrator() {
    let harness = Harness::new().await;
    let carol = harness.sign_in("carol-subject", "carol").await;
    let rule = harness.rule(monthly_pages(1, Enforcement::Hard)).await;
    let response = harness
        .send(page("/administration/quotas", Some(&carol)))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let carol_id = harness.user_id("carol").await;
    let user_grant = format!("/administration/users/{carol_id}/grant");
    let user_reset = format!("/administration/users/{carol_id}/reset");
    let rule_path = format!("/administration/quotas/{rule}");
    let grant = format!("{rule_path}/grant");
    let reset = format!("{rule_path}/reset");
    let enabled = format!("{rule_path}/enabled");
    let delete = format!("{rule_path}/delete");
    for path in [
        "/administration/quotas",
        rule_path.as_str(),
        enabled.as_str(),
        delete.as_str(),
        grant.as_str(),
        reset.as_str(),
        user_grant.as_str(),
        user_reset.as_str(),
    ] {
        let response = harness
            .send(form(
                path,
                Some(&carol),
                &[
                    ("confirm", "yes"),
                    ("enabled", "false"),
                    ("rule", &rule.to_string()),
                    ("amount", "5"),
                    ("reason", "because"),
                ],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let rules = quotas::rules(&harness.database).await.unwrap();
    assert_eq!(rules.len(), 1);
    assert!(rules[0].enabled);
}
