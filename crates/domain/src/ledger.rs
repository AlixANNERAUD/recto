//! The print ledger: one row of `print_jobs` per job, written at submission with the counts the
//! quotas and prices are computed from.
//!
//! The row exists before CUPS answers, so that a job can never print without being counted; it
//! gets its CUPS job id afterwards, or is refunded when CUPS refuses it. This module is the only
//! writer of new `print_jobs` rows.

use chrono::{DateTime, Duration, Utc};
use recto_entity::print_jobs;
use recto_printing::{JobState, JobStatus};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
};

use crate::error::Result;

/// What one job counts for, derived once at submission. `None` is an unknown count (a document
/// recto could not read), which is different from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    /// Impressions: pages printed, copies included.
    pub pages: Option<i64>,
    /// Sheets of paper, copies included.
    pub sheets: Option<i64>,
    pub color_pages: Option<i64>,
    pub mono_pages: Option<i64>,
    /// In minor currency units.
    pub cost: i64,
}

/// A printer's prices in minor currency units; free by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prices {
    pub mono_page: i64,
    pub color_page: i64,
    pub sheet: i64,
}

/// The most any count of one job can be. A job counting more is past any limit already, and
/// the cap keeps the ledger's sums within 64 bits: millions of such jobs in one window would be
/// needed to overflow them.
pub const MAX_COUNT: i64 = 1_000_000_000_000;

/// The counts of a job. `document_pages` are the pages the job prints once its page ranges are
/// applied, `None` when unknown; pages and sheets then stay unknown and cost nothing.
pub fn counts(
    document_pages: Option<u32>,
    copies: u32,
    two_sided: bool,
    color: bool,
    prices: Prices,
) -> Counts {
    let Some(document_pages) = document_pages else {
        return Counts {
            pages: None,
            sheets: None,
            color_pages: None,
            mono_pages: None,
            cost: 0,
        };
    };
    let document_pages = i64::from(document_pages);
    let copies = i64::from(copies);
    let pages = document_pages.saturating_mul(copies).min(MAX_COUNT);
    let per_copy = if two_sided {
        (document_pages + 1) / 2
    } else {
        document_pages
    };
    let sheets = per_copy.saturating_mul(copies).min(MAX_COUNT);
    let (color_pages, mono_pages) = if color { (pages, 0) } else { (0, pages) };
    let cost = mono_pages
        .saturating_mul(prices.mono_page)
        .saturating_add(color_pages.saturating_mul(prices.color_page))
        .saturating_add(sheets.saturating_mul(prices.sheet))
        .min(MAX_COUNT);
    Counts {
        pages: Some(pages),
        sheets: Some(sheets),
        color_pages: Some(color_pages),
        mono_pages: Some(mono_pages),
        cost,
    }
}

/// A job to enter in the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEntry {
    pub user_id: i64,
    /// Copied, so the history keeps the name the user had.
    pub username: String,
    pub printer: String,
    pub document_name: String,
    /// The MIME type of the document.
    pub document_format: String,
    /// The pages of the document itself, before page ranges; `None` when unknown.
    pub document_pages: Option<u32>,
    pub copies: u32,
    /// The IPP `sides` keyword.
    pub sides: String,
    /// The IPP `print-color-mode` keyword.
    pub color_mode: String,
    pub counts: Counts,
}

/// Writes a job in the `pending` state, with no CUPS job id yet; returns its id.
pub async fn insert(
    database: &impl ConnectionTrait,
    entry: NewEntry,
    now: DateTime<Utc>,
) -> Result<i64> {
    let row = print_jobs::ActiveModel {
        user_id: Set(entry.user_id),
        username: Set(entry.username),
        printer: Set(entry.printer),
        cups_job_id: Set(None),
        document_name: Set(entry.document_name),
        document_format: Set(Some(entry.document_format)),
        document_pages: Set(entry.document_pages.map(i64::from)),
        copies: Set(i64::from(entry.copies)),
        sides: Set(Some(entry.sides)),
        color_mode: Set(Some(entry.color_mode)),
        pages: Set(entry.counts.pages),
        sheets: Set(entry.counts.sheets),
        color_pages: Set(entry.counts.color_pages),
        mono_pages: Set(entry.counts.mono_pages),
        cost: Set(entry.counts.cost),
        refunded: Set(false),
        finished: Set(false),
        seen_processing: Set(false),
        impressions_completed: Set(None),
        submitted_at: Set(now.fixed_offset()),
        state: Set("pending".to_owned()),
        ..Default::default()
    }
    .insert(database)
    .await?;
    Ok(row.id)
}

/// Stores the id CUPS gave the job. It also takes back a refund by [`refund_unattached`]: an
/// attach that comes after the sweep proves CUPS has the job, so it counts again and the poll
/// follows it.
pub async fn attach(database: &impl ConnectionTrait, id: i64, cups_job_id: i32) -> Result<()> {
    print_jobs::Entity::update_many()
        .col_expr(print_jobs::Column::CupsJobId, i64::from(cups_job_id).into())
        .col_expr(print_jobs::Column::Refunded, false.into())
        .col_expr(print_jobs::Column::Finished, false.into())
        .filter(print_jobs::Column::Id.eq(id))
        .exec(database)
        .await?;
    Ok(())
}

/// Takes the job out of the counts: CUPS refused it, so nothing printed.
pub async fn refund(database: &impl ConnectionTrait, id: i64) -> Result<()> {
    print_jobs::Entity::update_many()
        .col_expr(print_jobs::Column::Refunded, true.into())
        .col_expr(print_jobs::Column::Finished, true.into())
        .filter(print_jobs::Column::Id.eq(id))
        .exec(database)
        .await?;
    Ok(())
}

/// How long an entry may wait for its CUPS job id. Submission attaches the id as soon as CUPS
/// answers, so an entry still without one after this long had its attach fail (a database
/// error) or recto stopped between the insert and the attach; [`refund_unattached`] sweeps it.
pub const ATTACH_GRACE: Duration = Duration::minutes(10);

/// The entries CUPS may still be working on, attached and not final, with their CUPS job id.
pub async fn unfinished(database: &impl ConnectionTrait) -> Result<Vec<(print_jobs::Model, i32)>> {
    let rows = print_jobs::Entity::find()
        .filter(print_jobs::Column::Finished.eq(false))
        .filter(print_jobs::Column::CupsJobId.is_not_null())
        .all(database)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let cups_job_id = i32::try_from(row.cups_job_id?).ok()?;
            Some((row, cups_job_id))
        })
        .collect())
}

/// Records what CUPS says of an entry's job, `row` being the entry as [`unfinished`] read it. A
/// job that printed nothing is refunded; `None` (CUPS forgot the job) only finishes the entry,
/// because nothing says it did not print. The poll sees most jobs unchanged, so only the columns
/// that change are written, and nothing when none does.
pub async fn observe(
    database: &impl ConnectionTrait,
    row: print_jobs::Model,
    status: Option<&JobStatus>,
) -> Result<()> {
    let seen_before = row.seen_processing;
    let mut entry: print_jobs::ActiveModel = row.into();
    match status {
        // Nothing says that it did not print.
        None => entry.finished.set_if_not_equals(true),
        Some(status) => {
            entry
                .state
                .set_if_not_equals(status.state.keyword().to_owned());
            if let Some(impressions) = status.impressions_completed {
                entry
                    .impressions_completed
                    .set_if_not_equals(Some(i64::from(impressions)));
            }
            let seen_processing =
                seen_before || matches!(status.state, JobState::Processing | JobState::Stopped);
            entry.seen_processing.set_if_not_equals(seen_processing);
            let printed_nothing = status.impressions_completed.unwrap_or(0) == 0;
            match status.state {
                JobState::Completed => entry.finished.set_if_not_equals(true),
                JobState::Canceled | JobState::Aborted => {
                    entry.finished.set_if_not_equals(true);
                    entry
                        .refunded
                        .set_if_not_equals(printed_nothing && !seen_processing);
                }
                _ => {}
            }
        }
    }
    if entry.is_changed() {
        entry.update(database).await?;
    }
    Ok(())
}

/// Refunds the entries that never got a CUPS job id within [`ATTACH_GRACE`]: CUPS most likely
/// never received the job, and such an entry would count against the quotas forever. Returns how
/// many were refunded.
pub async fn refund_unattached(database: &impl ConnectionTrait, now: DateTime<Utc>) -> Result<u64> {
    let result = print_jobs::Entity::update_many()
        .col_expr(print_jobs::Column::Refunded, true.into())
        .col_expr(print_jobs::Column::Finished, true.into())
        .filter(print_jobs::Column::Finished.eq(false))
        .filter(print_jobs::Column::CupsJobId.is_null())
        .filter(print_jobs::Column::SubmittedAt.lt((now - ATTACH_GRACE).fixed_offset()))
        .exec(database)
        .await?;
    Ok(result.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::{Identity, sign_in_with_identity};
    use chrono::Utc;
    use recto_entity::print_jobs;
    use recto_printing::{JobState, JobStatus};
    use sea_orm::EntityTrait;

    fn entry(user_id: i64) -> NewEntry {
        NewEntry {
            user_id,
            username: "alice".to_owned(),
            printer: "office".to_owned(),
            document_name: "report.pdf".to_owned(),
            document_format: "application/pdf".to_owned(),
            document_pages: Some(5),
            copies: 2,
            sides: "two-sided-long-edge".to_owned(),
            color_mode: "color".to_owned(),
            counts: counts(Some(5), 2, true, true, Prices::default()),
        }
    }

    #[test]
    fn counts_are_capped() {
        let dear = Prices {
            mono_page: i64::MAX,
            color_page: i64::MAX,
            sheet: i64::MAX,
        };
        let most = counts(Some(u32::MAX), u32::MAX, false, true, dear);
        assert_eq!(
            most,
            Counts {
                pages: Some(MAX_COUNT),
                sheets: Some(MAX_COUNT),
                color_pages: Some(MAX_COUNT),
                mono_pages: Some(0),
                cost: MAX_COUNT,
            }
        );
    }

    #[test]
    fn counts_follow_the_spec() {
        let free = Prices::default();
        assert_eq!(
            counts(Some(5), 2, true, true, free),
            Counts {
                pages: Some(10),
                sheets: Some(6),
                color_pages: Some(10),
                mono_pages: Some(0),
                cost: 0
            }
        );
        let mono = counts(Some(5), 2, false, false, free);
        assert_eq!(mono.sheets, Some(10));
        assert_eq!(mono.mono_pages, Some(10));
        assert_eq!(mono.color_pages, Some(0));
        assert_eq!(
            counts(
                None,
                3,
                true,
                true,
                Prices {
                    mono_page: 5,
                    color_page: 20,
                    sheet: 2
                }
            ),
            Counts {
                pages: None,
                sheets: None,
                color_pages: None,
                mono_pages: None,
                cost: 0
            }
        );
        let priced = Prices {
            mono_page: 5,
            color_page: 20,
            sheet: 2,
        };
        assert_eq!(counts(Some(3), 1, false, true, priced).cost, 3 * 20 + 3 * 2);
        assert_eq!(counts(Some(3), 1, false, false, priced).cost, 3 * 5 + 3 * 2);
    }

    #[tokio::test]
    async fn an_entry_is_written_before_cups_answers() {
        let database = crate::testing::database().await;
        let identity = Identity {
            issuer: "https://id.example".to_owned(),
            subject: "alice".to_owned(),
            preferred_username: Some("alice".to_owned()),
            ..Identity::default()
        };
        let alice = sign_in_with_identity(&database, &identity).await.unwrap();

        let id = insert(&database, entry(alice.id), Utc::now())
            .await
            .unwrap();
        let row = print_jobs::Entity::find_by_id(id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.cups_job_id, None);
        assert_eq!(row.state, "pending");
        assert_eq!(row.username, "alice");
        assert_eq!(row.pages, Some(10));
        assert_eq!(row.sheets, Some(6));
        assert_eq!(row.document_pages, Some(5));
        assert!(!row.refunded && !row.finished);

        attach(&database, id, 12).await.unwrap();
        let row = print_jobs::Entity::find_by_id(id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.cups_job_id, Some(12));

        refund(&database, id).await.unwrap();
        let row = print_jobs::Entity::find_by_id(id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert!(row.refunded && row.finished);
    }

    async fn attached(database: &sea_orm::DatabaseConnection, cups_job_id: i32) -> i64 {
        let identity = Identity {
            issuer: "https://id.example".to_owned(),
            subject: "alice".to_owned(),
            preferred_username: Some("alice".to_owned()),
            ..Identity::default()
        };
        let alice = sign_in_with_identity(database, &identity).await.unwrap();
        let id = insert(database, entry(alice.id), Utc::now()).await.unwrap();
        attach(database, id, cups_job_id).await.unwrap();
        id
    }

    async fn row(database: &sea_orm::DatabaseConnection, id: i64) -> print_jobs::Model {
        print_jobs::Entity::find_by_id(id)
            .one(database)
            .await
            .unwrap()
            .unwrap()
    }

    fn status(state: JobState, impressions: Option<u32>) -> JobStatus {
        JobStatus {
            state,
            impressions_completed: impressions,
        }
    }

    #[tokio::test]
    async fn observing_a_job_through_to_completion_keeps_its_counts() {
        let database = crate::testing::database().await;
        let id = attached(&database, 5).await;
        let listed = unfinished(&database).await.unwrap();
        assert_eq!(listed, vec![(row(&database, id).await, 5)]);

        observe(
            &database,
            row(&database, id).await,
            Some(&status(JobState::Pending, None)),
        )
        .await
        .unwrap();
        let pending = row(&database, id).await;
        assert!(!pending.finished && !pending.seen_processing);
        assert_eq!(pending.state, "pending");

        observe(
            &database,
            row(&database, id).await,
            Some(&status(JobState::Processing, Some(1))),
        )
        .await
        .unwrap();
        let processing = row(&database, id).await;
        assert!(!processing.finished && processing.seen_processing);
        assert_eq!(processing.state, "processing");

        observe(
            &database,
            row(&database, id).await,
            Some(&status(JobState::Completed, Some(10))),
        )
        .await
        .unwrap();
        let done = row(&database, id).await;
        assert!(done.finished && !done.refunded);
        assert_eq!(done.state, "completed");
        assert_eq!(done.impressions_completed, Some(10));
        assert_eq!(done.pages, Some(10));
        assert!(unfinished(&database).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_job_that_printed_nothing_is_refunded() {
        for (state, impressions) in [
            (JobState::Canceled, Some(0)),
            (JobState::Canceled, None),
            (JobState::Aborted, Some(0)),
        ] {
            let database = crate::testing::database().await;
            let id = attached(&database, 5).await;
            observe(
                &database,
                row(&database, id).await,
                Some(&status(state, impressions)),
            )
            .await
            .unwrap();
            let job = row(&database, id).await;
            assert!(job.finished && job.refunded, "{state:?} {impressions:?}");
            assert_eq!(job.state, state.keyword());
        }
    }

    #[tokio::test]
    async fn a_job_that_printed_something_keeps_its_counts() {
        // Canceled after it reached processing, aborted with pages out, canceled with pages out
        // that were never seen processing.
        for (seen, state, impressions) in [
            (true, JobState::Canceled, Some(0)),
            (true, JobState::Canceled, None),
            (false, JobState::Aborted, Some(2)),
            (false, JobState::Canceled, Some(1)),
            (false, JobState::Completed, Some(0)),
            (false, JobState::Completed, None),
        ] {
            let database = crate::testing::database().await;
            let id = attached(&database, 5).await;
            if seen {
                observe(
                    &database,
                    row(&database, id).await,
                    Some(&status(JobState::Processing, None)),
                )
                .await
                .unwrap();
            }
            observe(
                &database,
                row(&database, id).await,
                Some(&status(state, impressions)),
            )
            .await
            .unwrap();
            let job = row(&database, id).await;
            assert!(
                job.finished && !job.refunded,
                "{seen} {state:?} {impressions:?}"
            );
            assert_eq!(job.impressions_completed, impressions.map(i64::from));
        }
    }

    #[tokio::test]
    async fn observing_an_unchanged_job_writes_nothing() {
        let database = crate::testing::database().await;
        let id = attached(&database, 5).await;
        let printing = || status(JobState::Processing, Some(1));
        observe(&database, row(&database, id).await, Some(&printing()))
            .await
            .unwrap();
        let seen = row(&database, id).await;
        // Without its row, any write fails.
        print_jobs::Entity::delete_by_id(id)
            .exec(&database)
            .await
            .unwrap();

        observe(&database, seen.clone(), Some(&printing()))
            .await
            .unwrap();
        let done = status(JobState::Completed, Some(1));
        assert!(observe(&database, seen, Some(&done)).await.is_err());
    }

    #[tokio::test]
    async fn a_job_cups_forgot_is_finished_without_a_refund() {
        let database = crate::testing::database().await;
        let id = attached(&database, 5).await;
        observe(&database, row(&database, id).await, None)
            .await
            .unwrap();
        let job = row(&database, id).await;
        assert!(job.finished && !job.refunded);
        assert_eq!(job.state, "pending");
    }

    #[tokio::test]
    async fn an_entry_cups_never_heard_of_is_refunded_after_ten_minutes() {
        let database = crate::testing::database().await;
        let identity = Identity {
            issuer: "https://id.example".to_owned(),
            subject: "alice".to_owned(),
            preferred_username: Some("alice".to_owned()),
            ..Identity::default()
        };
        let alice = sign_in_with_identity(&database, &identity).await.unwrap();
        let now = Utc::now();
        let old = insert(
            &database,
            entry(alice.id),
            now - chrono::Duration::minutes(11),
        )
        .await
        .unwrap();
        let recent = insert(
            &database,
            entry(alice.id),
            now - chrono::Duration::minutes(9),
        )
        .await
        .unwrap();
        let sent = insert(
            &database,
            entry(alice.id),
            now - chrono::Duration::minutes(30),
        )
        .await
        .unwrap();
        attach(&database, sent, 3).await.unwrap();

        assert_eq!(refund_unattached(&database, now).await.unwrap(), 1);

        assert!(row(&database, old).await.refunded);
        assert!(row(&database, old).await.finished);
        assert!(!row(&database, recent).await.refunded);
        assert!(!row(&database, sent).await.refunded);
        assert_eq!(refund_unattached(&database, now).await.unwrap(), 0);

        // CUPS had the job after all: a late attach makes it count again.
        attach(&database, old, 8).await.unwrap();
        let late = row(&database, old).await;
        assert!(!late.refunded && !late.finished);
        assert_eq!(late.cups_job_id, Some(8));
    }

    #[tokio::test]
    async fn jobs_recorded_before_the_ledger_are_finished_afterwards() {
        use recto_migration::{Migrator, MigratorTrait};
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

        let url = format!(
            "sqlite:file:recto-ledger-migration-{}?mode=memory&cache=shared",
            std::process::id()
        );
        let database = crate::database::connect(&url).await.unwrap();
        // Every migration before the ledger's.
        let before = Migrator::migrations()
            .iter()
            .position(|migration| migration.name() == "m20261117_000001_ledger")
            .expect("the ledger migration") as u32;
        Migrator::up(&database, Some(before)).await.unwrap();
        for sql in [
            "INSERT INTO users (username, display_name, is_administrator, administrator_from_claim, \
             is_disabled, created_at) VALUES ('old', 'Old', 0, 0, 0, '2026-01-01T00:00:00+00:00')",
            "INSERT INTO print_jobs (user_id, printer, cups_job_id, document_name, submitted_at, state) \
             VALUES (1, 'office', 4, 'a.pdf', '2026-01-02T00:00:00+00:00', 'completed')",
            "INSERT INTO print_jobs (user_id, printer, cups_job_id, document_name, submitted_at, state) \
             VALUES (1, 'office', NULL, 'b.pdf', '2026-01-02T00:00:00+00:00', 'pending')",
        ] {
            database.execute_unprepared(sql).await.unwrap();
        }

        Migrator::up(&database, None).await.unwrap();

        let rows = database
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT finished, pages, cost, refunded FROM print_jobs",
            ))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert!(row.try_get::<bool>("", "finished").unwrap());
            assert_eq!(row.try_get::<Option<i64>>("", "pages").unwrap(), None);
            assert_eq!(row.try_get::<i64>("", "cost").unwrap(), 0);
            assert!(!row.try_get::<bool>("", "refunded").unwrap());
        }
    }
}
