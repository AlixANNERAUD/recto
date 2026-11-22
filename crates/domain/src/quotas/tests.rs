//! The quota functions on a database: rules, usage, the check and adjustments.

use chrono::{DateTime, Utc};
use chrono_tz::{Europe::Paris, Tz};
use recto_entity::{groups, quota_adjustments, quota_rules, users};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use super::*;
use crate::{
    database::begin_write,
    ledger::{self, Counts, NewEntry, Prices},
    users::{Identity, sign_in_with_identity},
};

const TZ: Tz = Paris;

fn utc(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
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
    groups::Entity::find()
        .filter(groups::Column::Name.eq(name))
        .one(db)
        .await
        .unwrap()
        .expect("the group")
        .id
}

fn counts(pages: Option<i64>, color: bool) -> Counts {
    let pages = pages.map(|pages| u32::try_from(pages).unwrap());
    ledger::counts(pages, 1, false, color, Prices::default())
}

/// Enters a job of `pages` pages (`None`: unknown) in the ledger at `at`; returns its id.
async fn printed(
    db: &DatabaseConnection,
    user_id: i64,
    printer: &str,
    pages: Option<i64>,
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
            document_pages: pages.map(|pages| u32::try_from(pages).unwrap()),
            copies: 1,
            sides: "one-sided".to_owned(),
            color_mode: "monochrome".to_owned(),
            counts: counts(pages, false),
        },
        utc(at),
    )
    .await
    .unwrap()
}

fn new_rule(subject: Subject, metric: Metric, period: Period, limit: i64) -> NewRule {
    NewRule {
        name: "Monthly pages".to_owned(),
        enabled: true,
        subject,
        printers: vec![],
        metric,
        period,
        limit,
        enforcement: Enforcement::Hard,
    }
}

async fn created(db: &DatabaseConnection, rule: NewRule) -> Rule {
    let id = create(db, rule).await.unwrap();
    super::rule(db, id).await.unwrap()
}

#[tokio::test]
async fn rules_round_trip() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &["design"]).await;
    let design = group_id(&db, "design").await;
    assert!(rules(&db).await.unwrap().is_empty());

    let mut lab = new_rule(
        Subject::Pool(design),
        Metric::Cost,
        Period::Rolling(30),
        1500,
    );
    lab.name = "  Lab budget ".to_owned();
    lab.printers = vec![" Lab".into(), "office".into(), "LAB".into()];
    lab.enforcement = Enforcement::Soft;
    let lab_id = create(&db, lab).await.unwrap();
    let mine = create(
        &db,
        new_rule(Subject::User(alice), Metric::Jobs, Period::Total, 3),
    )
    .await
    .unwrap();

    let all = rules(&db).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[0],
        Rule {
            id: lab_id,
            name: "Lab budget".to_owned(),
            enabled: true,
            subject: Subject::Pool(design),
            // Trimmed, once each whatever the case, sorted.
            printers: vec!["Lab".into(), "office".into()],
            metric: Metric::Cost,
            period: Period::Rolling(30),
            limit: 1500,
            enforcement: Enforcement::Soft,
        }
    );
    assert_eq!(all[1].subject, Subject::User(alice));
    assert_eq!(all[1].period, Period::Total);

    let mut changed = new_rule(Subject::Everyone, Metric::ColorPages, Period::Quarter, 70);
    changed.enabled = false;
    update(&db, lab_id, changed).await.unwrap();
    let rule = super::rule(&db, lab_id).await.unwrap();
    assert_eq!(rule.subject, Subject::Everyone);
    assert!(rule.printers.is_empty() && !rule.enabled);
    assert_eq!(
        (rule.metric, rule.period, rule.limit),
        (Metric::ColorPages, Period::Quarter, 70)
    );

    delete(&db, mine).await.unwrap();
    assert_eq!(rules(&db).await.unwrap().len(), 1);
    assert!(matches!(delete(&db, mine).await, Err(Error::NotFound)));
    assert!(matches!(
        update(
            &db,
            mine,
            new_rule(Subject::Everyone, Metric::Pages, Period::Day, 1)
        )
        .await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn invalid_rules_are_refused() {
    let db = crate::testing::database().await;
    let good = new_rule(Subject::Everyone, Metric::Pages, Period::Month, 10);
    let cases = [
        (
            NewRule {
                name: "  ".into(),
                ..good.clone()
            },
            Problem::RuleNameEmpty,
        ),
        (
            NewRule {
                limit: -1,
                ..good.clone()
            },
            Problem::RuleLimitNegative,
        ),
        (
            NewRule {
                period: Period::Rolling(0),
                ..good.clone()
            },
            Problem::RollingDaysInvalid { maximum: 366 },
        ),
        (
            NewRule {
                period: Period::Rolling(367),
                ..good.clone()
            },
            Problem::RollingDaysInvalid { maximum: 366 },
        ),
        (
            NewRule {
                subject: Subject::Group(999),
                ..good.clone()
            },
            Problem::SubjectNotFound,
        ),
        (
            NewRule {
                subject: Subject::Pool(999),
                ..good.clone()
            },
            Problem::SubjectNotFound,
        ),
        (
            NewRule {
                subject: Subject::User(999),
                ..good.clone()
            },
            Problem::SubjectNotFound,
        ),
        (
            NewRule {
                printers: vec!["office".into(), " ".into()],
                ..good.clone()
            },
            Problem::PrinterNameEmpty,
        ),
    ];
    let id = create(&db, good.clone()).await.unwrap();
    for (rule, problem) in cases {
        match create(&db, rule.clone()).await {
            Err(Error::Invalid(refused)) => assert_eq!(refused, problem),
            other => panic!("{rule:?}: {other:?}"),
        }
        match update(&db, id, rule.clone()).await {
            Err(Error::Invalid(refused)) => assert_eq!(refused, problem),
            other => panic!("{rule:?}: {other:?}"),
        }
    }
    assert_eq!(rules(&db).await.unwrap().len(), 1);
    // The limits: 0 and 366 days are fine.
    create(
        &db,
        NewRule {
            limit: 0,
            period: Period::Rolling(366),
            ..good
        },
    )
    .await
    .unwrap();

    // Adjustments: a positive amount, for whom the rule is.
    let alice = person(&db, "alice", &["design"]).await;
    let design = group_id(&db, "design").await;
    let now = utc("2026-11-17T10:00:00Z");
    let refused = |result: Result<()>| match result {
        Err(Error::Invalid(problem)) => problem,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        refused(grant(&db, id, Holder::User(alice), 0, "", alice, now, TZ).await),
        Problem::GrantNotPositive
    );
    assert_eq!(
        refused(grant(&db, id, Holder::Group(design), 5, "", alice, now, TZ).await),
        Problem::HolderMismatch
    );
    assert_eq!(
        refused(grant(&db, id, Holder::User(999), 5, "", alice, now, TZ).await),
        Problem::SubjectNotFound
    );
    let pool = create(
        &db,
        new_rule(Subject::Pool(design), Metric::Pages, Period::Month, 10),
    )
    .await
    .unwrap();
    assert_eq!(
        refused(reset(&db, pool, Holder::User(alice), "", alice, now).await),
        Problem::HolderMismatch
    );
    assert!(matches!(
        reset(&db, 999, Holder::User(alice), "", alice, now).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn limits_and_grants_are_capped() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &[]).await;
    let now = utc("2026-11-17T10:00:00Z");
    let refused = |result: Result<i64>| match result {
        Err(Error::Invalid(problem)) => problem,
        other => panic!("{other:?}"),
    };
    let rule = |limit| new_rule(Subject::Everyone, Metric::Pages, Period::Month, limit);
    assert_eq!(
        refused(create(&db, rule(MAX_AMOUNT + 1)).await),
        Problem::RuleLimitTooLarge {
            maximum: MAX_AMOUNT
        }
    );
    let id = create(&db, rule(MAX_AMOUNT)).await.unwrap();
    assert!(matches!(
        update(&db, id, rule(i64::MAX)).await,
        Err(Error::Invalid(Problem::RuleLimitTooLarge { .. }))
    ));
    assert!(matches!(
        grant(
            &db,
            id,
            Holder::User(alice),
            MAX_AMOUNT + 1,
            "",
            alice,
            now,
            TZ
        )
        .await,
        Err(Error::Invalid(Problem::GrantTooLarge {
            maximum: MAX_AMOUNT
        }))
    ));
    // The most there can be still sums without overflowing.
    for _ in 0..3 {
        grant(&db, id, Holder::User(alice), MAX_AMOUNT, "", alice, now, TZ)
            .await
            .unwrap();
    }
    let rule = super::rule(&db, id).await.unwrap();
    let standing = standing(&db, &rule, alice, now, TZ).await.unwrap();
    assert_eq!(standing.granted, 3 * MAX_AMOUNT);
    assert_eq!(standing.left, 4 * MAX_AMOUNT);
}

#[tokio::test]
async fn an_adjusted_rule_keeps_its_metric_and_period() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &[]).await;
    let now = utc("2026-11-17T10:00:00Z");
    let pages = new_rule(Subject::Everyone, Metric::Pages, Period::Rolling(7), 10);
    let id = create(&db, pages.clone()).await.unwrap();
    // Before any adjustment, anything may change.
    let cost = NewRule {
        metric: Metric::Cost,
        period: Period::Month,
        ..pages.clone()
    };
    update(&db, id, cost).await.unwrap();
    update(&db, id, pages.clone()).await.unwrap();

    reset(&db, id, Holder::User(alice), "", alice, now)
        .await
        .unwrap();
    for changed in [
        NewRule {
            metric: Metric::Cost,
            ..pages.clone()
        },
        NewRule {
            period: Period::Month,
            ..pages.clone()
        },
    ] {
        assert!(
            matches!(
                update(&db, id, changed.clone()).await,
                Err(Error::Invalid(Problem::RuleAdjusted))
            ),
            "{changed:?}"
        );
    }
    // The rest still changes, the days of a rolling period included.
    let changed = NewRule {
        name: "Weekly-ish".into(),
        period: Period::Rolling(14),
        limit: 20,
        enabled: false,
        ..pages
    };
    update(&db, id, changed).await.unwrap();
    let rule = super::rule(&db, id).await.unwrap();
    assert_eq!(
        (rule.metric, rule.period, rule.limit),
        (Metric::Pages, Period::Rolling(14), 20)
    );
}

#[tokio::test]
async fn usage_counts_the_window_after_the_latest_reset_minus_grants() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &[]).await;
    let bob = person(&db, "bob", &[]).await;
    let rule = created(
        &db,
        new_rule(Subject::Everyone, Metric::Pages, Period::Month, 100),
    )
    .await;
    let now = utc("2026-11-17T10:00:00Z");

    // 31 October, 23:30 in Paris: last month.
    printed(&db, alice, "office", Some(50), "2026-10-31T22:30:00Z").await;
    // Before the latest reset.
    printed(&db, alice, "office", Some(5), "2026-11-02T10:00:00Z").await;
    let refunded = printed(&db, alice, "office", Some(40), "2026-11-10T10:00:00Z").await;
    ledger::refund(&db, refunded).await.unwrap();
    printed(&db, alice, "office", Some(7), "2026-11-12T10:00:00Z").await;
    printed(&db, alice, "office", None, "2026-11-12T11:00:00Z").await;
    printed(&db, bob, "office", Some(9), "2026-11-12T10:00:00Z").await;

    let standing_now = || async { standing(&db, &rule, alice, now, TZ).await.unwrap() };
    let before = standing_now().await;
    assert_eq!((before.used, before.granted, before.left), (12, 0, 88));

    reset(
        &db,
        rule.id,
        Holder::User(alice),
        "old",
        bob,
        utc("2026-11-01T12:00:00Z"),
    )
    .await
    .unwrap();
    reset(
        &db,
        rule.id,
        Holder::User(alice),
        "paid",
        bob,
        utc("2026-11-05T00:00:00Z"),
    )
    .await
    .unwrap();
    // Bob's reset is his.
    reset(
        &db,
        rule.id,
        Holder::User(bob),
        "",
        bob,
        utc("2026-11-16T00:00:00Z"),
    )
    .await
    .unwrap();
    // A grant of last month has expired; this month's counts.
    grant(
        &db,
        rule.id,
        Holder::User(alice),
        20,
        "",
        bob,
        utc("2026-10-15T10:00:00Z"),
        TZ,
    )
    .await
    .unwrap();
    grant(
        &db,
        rule.id,
        Holder::User(alice),
        10,
        "exam",
        bob,
        utc("2026-11-10T10:00:00Z"),
        TZ,
    )
    .await
    .unwrap();

    let after = standing_now().await;
    assert_eq!(after.rule, rule);
    // The unknown job counts nothing.
    assert_eq!((after.used, after.granted, after.left), (7, 10, 103));
    assert_eq!(standing(&db, &rule, bob, now, TZ).await.unwrap().used, 0);

    // The jobs metric counts the unknown job too, and only Lab jobs count on a Lab rule.
    let mut jobs = new_rule(Subject::Everyone, Metric::Jobs, Period::Month, 5);
    jobs.printers = vec!["LAB".into()];
    let jobs = created(&db, jobs).await;
    assert_eq!(standing(&db, &jobs, alice, now, TZ).await.unwrap().used, 0);
    printed(&db, alice, "lab", None, "2026-11-13T10:00:00Z").await;
    printed(&db, alice, "Lab", Some(3), "2026-11-13T11:00:00Z").await;
    let lab = standing(&db, &jobs, alice, now, TZ).await.unwrap();
    assert_eq!((lab.used, lab.left), (2, 3));
    // The month rule counts them as pages.
    assert_eq!(standing_now().await.used, 10);

    // A rolling window and a total one.
    let week = created(
        &db,
        new_rule(Subject::Everyone, Metric::Pages, Period::Rolling(7), 100),
    )
    .await;
    assert_eq!(standing(&db, &week, alice, now, TZ).await.unwrap().used, 10);
    let ever = created(
        &db,
        new_rule(Subject::Everyone, Metric::Pages, Period::Total, 1000),
    )
    .await;
    assert_eq!(standing(&db, &ever, alice, now, TZ).await.unwrap().used, 65);
}

#[tokio::test]
async fn usage_finds_printers_named_beyond_ascii() {
    // CUPS accepts UTF-8 printer names. PostgreSQL's LOWER folds É and SQLite's does not, so
    // both sides of the comparison must go through the same LOWER.
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &[]).await;
    let mut school = new_rule(Subject::Everyone, Metric::Pages, Period::Month, 100);
    school.printers = vec!["ÉCOLE".into()];
    let school = created(&db, school).await;
    printed(&db, alice, "ÉCOLE", Some(4), "2026-11-12T10:00:00Z").await;
    printed(&db, alice, "École", Some(2), "2026-11-12T11:00:00Z").await;
    printed(&db, alice, "office", Some(1), "2026-11-12T12:00:00Z").await;
    let now = utc("2026-11-17T10:00:00Z");
    assert_eq!(
        standing(&db, &school, alice, now, TZ).await.unwrap().used,
        6
    );
}

#[tokio::test]
async fn a_pool_sums_every_member() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &["design"]).await;
    let bob = person(&db, "bob", &["design", "sales"]).await;
    let carol = person(&db, "carol", &["sales"]).await;
    let design = group_id(&db, "design").await;
    let rule = created(
        &db,
        new_rule(Subject::Pool(design), Metric::Pages, Period::Month, 10),
    )
    .await;
    let now = utc("2026-11-17T10:00:00Z");
    printed(&db, alice, "office", Some(3), "2026-11-16T10:00:00Z").await;
    printed(&db, bob, "office", Some(4), "2026-11-16T11:00:00Z").await;
    printed(&db, carol, "office", Some(5), "2026-11-16T12:00:00Z").await;

    for member in [alice, bob] {
        let pool = standing(&db, &rule, member, now, TZ).await.unwrap();
        assert_eq!((pool.used, pool.left), (7, 3));
    }

    // Adjustments of a pool are the group's.
    grant(&db, rule.id, Holder::Group(design), 5, "", carol, now, TZ)
        .await
        .unwrap();
    assert_eq!(standing(&db, &rule, alice, now, TZ).await.unwrap().left, 8);
    reset(
        &db,
        rule.id,
        Holder::Group(design),
        "",
        carol,
        utc("2026-11-16T10:30:00Z"),
    )
    .await
    .unwrap();
    let pool = standing(&db, &rule, bob, now, TZ).await.unwrap();
    assert_eq!((pool.used, pool.granted, pool.left), (4, 5, 11));

    // Membership as recorded now: once bob leaves, his jobs leave the pool.
    person(&db, "bob", &["sales"]).await;
    let pool = standing(&db, &rule, alice, now, TZ).await.unwrap();
    assert_eq!(pool.used, 0);
}

async fn verdict(
    db: &DatabaseConnection,
    user: i64,
    groups: &[i64],
    printer: &str,
    counts: Counts,
    now: DateTime<Utc>,
) -> Verdict {
    let transaction = begin_write(db).await.unwrap();
    lock_subjects(&transaction, user, groups).await.unwrap();
    let verdict = check(&transaction, user, groups, printer, &counts, now, TZ)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    verdict
}

#[tokio::test]
async fn check_refuses_over_a_hard_rule_and_warns_over_a_soft_one() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &[]).await;
    let now = utc("2026-11-17T10:00:00Z");
    // Nothing to check without rules.
    assert_eq!(
        verdict(&db, alice, &[], "office", counts(Some(100), false), now).await,
        Verdict::Allowed { warnings: vec![] }
    );
    let rule = created(
        &db,
        new_rule(Subject::Everyone, Metric::Pages, Period::Month, 5),
    )
    .await;
    printed(&db, alice, "office", Some(3), "2026-11-16T10:00:00Z").await;

    match verdict(&db, alice, &[], "office", counts(Some(3), false), now).await {
        Verdict::Refused { standing } => {
            assert_eq!(standing.rule.id, rule.id);
            assert_eq!(standing.left, 2);
        }
        other => panic!("{other:?}"),
    }
    // Exactly what is left fits.
    assert_eq!(
        verdict(&db, alice, &[], "office", counts(Some(2), false), now).await,
        Verdict::Allowed { warnings: vec![] }
    );

    let soft = NewRule {
        enforcement: Enforcement::Soft,
        ..new_rule(Subject::Everyone, Metric::Pages, Period::Month, 5)
    };
    update(&db, rule.id, soft).await.unwrap();
    match verdict(&db, alice, &[], "office", counts(Some(3), false), now).await {
        Verdict::Allowed { warnings } => {
            assert_eq!(warnings.len(), 1);
            assert_eq!(warnings[0].left, 2);
        }
        other => panic!("{other:?}"),
    }

    // A color rule already over does not stop black and white, and two hard rules over
    // refuse with the first.
    let first = created(
        &db,
        new_rule(Subject::Everyone, Metric::ColorPages, Period::Month, 0),
    )
    .await;
    let second = created(
        &db,
        new_rule(Subject::Everyone, Metric::Jobs, Period::Month, 2),
    )
    .await;
    assert!(matches!(
        verdict(&db, alice, &[], "office", counts(Some(1), false), now).await,
        Verdict::Allowed { .. }
    ));
    printed(&db, alice, "office", Some(1), "2026-11-16T11:00:00Z").await;
    match verdict(&db, alice, &[], "office", counts(Some(1), true), now).await {
        Verdict::Refused { standing } => assert_eq!(standing.rule.id, first.id),
        other => panic!("{other:?}"),
    }
    match verdict(&db, alice, &[], "office", counts(Some(1), false), now).await {
        Verdict::Refused { standing } => assert_eq!(standing.rule.id, second.id),
        other => panic!("{other:?}"),
    }
    // A rule of another printer does not apply.
    delete(&db, second.id).await.unwrap();
    let mut lab = new_rule(Subject::Everyone, Metric::Pages, Period::Month, 0);
    lab.printers = vec!["lab".into()];
    create(&db, lab).await.unwrap();
    assert!(matches!(
        verdict(&db, alice, &[], "office", counts(Some(1), false), now).await,
        Verdict::Allowed { .. }
    ));
    assert!(matches!(
        verdict(&db, alice, &[], "LAB", counts(Some(1), false), now).await,
        Verdict::Refused { .. }
    ));
}

#[tokio::test]
async fn check_counts_pools_and_locks_their_groups() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &["design"]).await;
    let bob = person(&db, "bob", &["design"]).await;
    let design = group_id(&db, "design").await;
    let now = utc("2026-11-17T10:00:00Z");
    created(
        &db,
        new_rule(Subject::Pool(design), Metric::Pages, Period::Month, 10),
    )
    .await;
    // A personal allowance bigger than the pool does not lift it.
    created(
        &db,
        new_rule(Subject::Group(design), Metric::Pages, Period::Month, 100),
    )
    .await;
    printed(&db, alice, "office", Some(8), "2026-11-16T10:00:00Z").await;
    let all = rules(&db).await.unwrap();
    assert_eq!(
        pool_groups(&applicable(&all, bob, &[design], "office")),
        [design]
    );
    match verdict(&db, bob, &[design], "office", counts(Some(3), false), now).await {
        Verdict::Refused { standing } => {
            assert_eq!(standing.rule.subject, Subject::Pool(design));
            assert_eq!(standing.left, 2);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn grants_expire_with_their_window() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &[]).await;
    let at = utc("2026-11-17T10:00:00Z");
    let db = &db;
    let expiry = |period| async move {
        let rule = created(db, new_rule(Subject::Everyone, Metric::Pages, period, 1)).await;
        grant(
            db,
            rule.id,
            Holder::User(alice),
            5,
            "  exam  ",
            alice,
            at,
            TZ,
        )
        .await
        .unwrap();
        let row = quota_adjustments::Entity::find()
            .filter(quota_adjustments::Column::RuleId.eq(rule.id))
            .one(db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.kind, "grant");
        assert_eq!(row.amount, Some(5));
        assert_eq!(row.reason, "exam");
        assert_eq!(row.by_user_id, alice);
        assert_eq!(row.user_id, Some(alice));
        assert_eq!(row.group_id, None);
        (rule, row.expires_at.map(|time| time.with_timezone(&Utc)))
    };
    let (month, expires) = expiry(Period::Month).await;
    assert_eq!(expires, Some(utc("2026-11-30T23:00:00Z")));
    assert_eq!(
        standing(db, &month, alice, utc("2026-11-30T22:59:00Z"), TZ)
            .await
            .unwrap()
            .granted,
        5
    );
    assert_eq!(
        standing(db, &month, alice, utc("2026-11-30T23:00:00Z"), TZ)
            .await
            .unwrap()
            .granted,
        0
    );
    assert_eq!(
        expiry(Period::Rolling(7)).await.1,
        Some(utc("2026-11-24T10:00:00Z"))
    );
    let (total, expires) = expiry(Period::Total).await;
    assert_eq!(expires, None);
    assert_eq!(
        standing(db, &total, alice, utc("2036-01-01T00:00:00Z"), TZ)
            .await
            .unwrap()
            .granted,
        5
    );
    assert_eq!(
        grant_expiry(Period::Day, at, TZ),
        Some(utc("2026-11-17T23:00:00Z"))
    );
}

#[tokio::test]
async fn deleting_a_group_removes_its_rules() {
    let db = crate::testing::database().await;
    let alice = person(&db, "alice", &["design", "sales"]).await;
    let design = group_id(&db, "design").await;
    let sales = group_id(&db, "sales").await;
    let now = utc("2026-11-17T10:00:00Z");
    let group_rule = created(
        &db,
        new_rule(Subject::Group(design), Metric::Pages, Period::Month, 1),
    )
    .await;
    let pool = created(
        &db,
        new_rule(Subject::Pool(design), Metric::Pages, Period::Month, 1),
    )
    .await;
    let kept = [
        created(
            &db,
            new_rule(Subject::Group(sales), Metric::Pages, Period::Month, 1),
        )
        .await,
        created(
            &db,
            new_rule(Subject::Everyone, Metric::Pages, Period::Month, 1),
        )
        .await,
        created(
            &db,
            new_rule(Subject::User(alice), Metric::Pages, Period::Month, 1),
        )
        .await,
    ];
    grant(&db, pool.id, Holder::Group(design), 3, "", alice, now, TZ)
        .await
        .unwrap();
    grant(
        &db,
        group_rule.id,
        Holder::User(alice),
        3,
        "",
        alice,
        now,
        TZ,
    )
    .await
    .unwrap();

    crate::groups::delete(&db, design).await.unwrap();

    let ids: Vec<i64> = rules(&db)
        .await
        .unwrap()
        .iter()
        .map(|rule| rule.id)
        .collect();
    assert_eq!(ids, kept.iter().map(|rule| rule.id).collect::<Vec<_>>());
    assert!(
        quota_adjustments::Entity::find()
            .all(&db)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        groups::Entity::find_by_id(design)
            .one(&db)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        crate::groups::delete(&db, design).await,
        Err(Error::NotFound)
    ));

    // recto deletes no users, and the database keeps it so for a user with adjustments, which
    // are history like their print jobs.
    let rule = kept[2].id;
    grant(&db, rule, Holder::User(alice), 1, "", alice, now, TZ)
        .await
        .unwrap();
    assert!(users::Entity::delete_by_id(alice).exec(&db).await.is_err());
    assert_eq!(
        quota_rules::Entity::find()
            .order_by_asc(quota_rules::Column::Id)
            .all(&db)
            .await
            .unwrap()
            .len(),
        3
    );
}
