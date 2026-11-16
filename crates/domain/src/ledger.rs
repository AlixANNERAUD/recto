//! The print ledger: one row of `print_jobs` per job, written at submission with the counts the
//! quotas and prices are computed from.
//!
//! The row exists before CUPS answers, so that a job can never print without being counted; it
//! gets its CUPS job id afterwards, or is refunded when CUPS refuses it. This module is the only
//! writer of new `print_jobs` rows.

use chrono::{DateTime, Utc};
use recto_entity::print_jobs;
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
    let pages = document_pages * copies;
    let per_copy = if two_sided {
        (document_pages + 1) / 2
    } else {
        document_pages
    };
    let sheets = per_copy * copies;
    let (color_pages, mono_pages) = if color { (pages, 0) } else { (0, pages) };
    let cost = mono_pages
        .saturating_mul(prices.mono_page)
        .saturating_add(color_pages.saturating_mul(prices.color_page))
        .saturating_add(sheets.saturating_mul(prices.sheet));
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

/// Stores the id CUPS gave the job.
pub async fn attach(database: &impl ConnectionTrait, id: i64, cups_job_id: i32) -> Result<()> {
    print_jobs::Entity::update_many()
        .col_expr(print_jobs::Column::CupsJobId, i64::from(cups_job_id).into())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::{Identity, sign_in_with_identity};
    use chrono::Utc;
    use recto_entity::print_jobs;
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

    #[tokio::test]
    async fn jobs_recorded_before_the_ledger_are_finished_afterwards() {
        use recto_migration::{Migrator, MigratorTrait};
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

        let url = format!(
            "sqlite:file:recto-ledger-migration-{}?mode=memory&cache=shared",
            std::process::id()
        );
        let database = crate::database::connect(&url).await.unwrap();
        // Everything but the ledger migration, which is the last.
        let before = Migrator::migrations().len() as u32 - 1;
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
