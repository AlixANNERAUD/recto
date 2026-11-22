//! Reading the records of the print jobs recto submitted, which [`crate::ledger`] writes. CUPS
//! only knows the name recto printed in, so the records are how the job list tells the real users
//! apart.
//!
//! A CUPS job id is not unique forever (CUPS can restart its numbering), so a record only
//! matches a CUPS job when their times agree as well: see [`matching`].

use chrono::{DateTime, Duration, Utc};
use recto_entity::{print_jobs, users};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

use crate::error::Result;

/// How far apart a record's submission and the CUPS job's creation may be for them to be the
/// same job. CUPS stamps the job when it receives it, so the real gap is a few seconds; the rest
/// absorbs clock differences between recto and CUPS.
pub const MATCH_WINDOW: Duration = Duration::minutes(5);

/// A recorded job with the user who submitted it. Only records that have a CUPS job id are
/// read back: one without never matches a CUPS job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedJob {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub printer: String,
    pub cups_job_id: i32,
    pub submitted_at: DateTime<Utc>,
    pub state: String,
    /// Pages counted at submission, copies included; `None` when unknown.
    pub pages: Option<i64>,
    /// In minor currency units.
    pub cost: i64,
}

/// The records of these CUPS job ids, with their users.
pub async fn for_cups_ids(
    database: &impl ConnectionTrait,
    ids: &[i32],
) -> Result<Vec<RecordedJob>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = print_jobs::Entity::find()
        .filter(print_jobs::Column::CupsJobId.is_in(ids.iter().map(|id| i64::from(*id))))
        .find_also_related(users::Entity)
        .all(database)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(job, user)| {
            let user = user?;
            Some(RecordedJob {
                cups_job_id: i32::try_from(job.cups_job_id?).ok()?,
                id: job.id,
                user_id: job.user_id,
                username: user.username,
                display_name: user.display_name,
                printer: job.printer,
                submitted_at: job.submitted_at.with_timezone(&Utc),
                state: job.state,
                pages: job.pages,
                cost: job.cost,
            })
        })
        .collect())
}

/// The record of a CUPS job: the same id, submitted within [`MATCH_WINDOW`] of the job's
/// creation time (unix seconds) when CUPS reports one, the newest when several qualify.
pub fn matching(
    records: &[RecordedJob],
    cups_job_id: i32,
    created: Option<i64>,
) -> Option<&RecordedJob> {
    let created = created.and_then(|seconds| DateTime::from_timestamp(seconds, 0));
    records
        .iter()
        .filter(|record| record.cups_job_id == cups_job_id)
        .filter(|record| match created {
            Some(created) => (created - record.submitted_at).abs() <= MATCH_WINDOW,
            None => true,
        })
        .max_by_key(|record| record.submitted_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::{Identity, sign_in_with_identity};
    use sea_orm::DatabaseConnection;

    async fn user(database: &DatabaseConnection, name: &str) -> users::Model {
        let identity = Identity {
            issuer: "https://id.example".to_owned(),
            subject: name.to_owned(),
            preferred_username: Some(name.to_owned()),
            ..Identity::default()
        };
        sign_in_with_identity(database, &identity).await.unwrap()
    }

    async fn record(database: &DatabaseConnection, user_id: i64, cups_job_id: i32) {
        crate::testing::recorded_job(database, user_id, "office", cups_job_id, "report.pdf").await;
    }

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    fn recorded(cups_job_id: i32, submitted_at: &str) -> RecordedJob {
        RecordedJob {
            id: i64::from(cups_job_id),
            user_id: 1,
            username: "alice".to_owned(),
            display_name: "Alice".to_owned(),
            printer: "office".to_owned(),
            cups_job_id,
            submitted_at: at(submitted_at),
            state: "pending".to_owned(),
            pages: None,
            cost: 0,
        }
    }

    #[tokio::test]
    async fn a_recorded_job_comes_back_with_its_user() {
        let database = crate::testing::database().await;
        let alice = user(&database, "alice").await;
        record(&database, alice.id, 7).await;

        let found = for_cups_ids(&database, &[7, 8]).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].username, "alice");
        assert_eq!(found[0].cups_job_id, 7);
        assert_eq!(found[0].state, "pending");
        assert!(for_cups_ids(&database, &[]).await.unwrap().is_empty());
    }

    #[test]
    fn a_reused_job_id_does_not_match_an_old_record() {
        let january = recorded(7, "2026-01-01T00:00:00Z");
        let submitted = january.submitted_at.timestamp();
        let records = [january];
        let march = at("2026-03-01T00:00:00Z").timestamp();
        assert!(matching(&records, 7, Some(march)).is_none());
        assert!(matching(&records, 7, Some(submitted + 120)).is_some());
        assert!(matching(&records, 7, None).is_some());
        assert!(matching(&records, 8, None).is_none());

        let records = [
            recorded(7, "2026-01-01T00:00:00Z"),
            recorded(7, "2026-03-01T00:00:00Z"),
        ];
        let found = matching(&records, 7, Some(march + 5)).unwrap();
        assert_eq!(found.submitted_at, at("2026-03-01T00:00:00Z"));
        let newest = matching(&records, 7, None).unwrap();
        assert_eq!(newest.submitted_at, at("2026-03-01T00:00:00Z"));
    }

    #[test]
    fn the_match_window_ends_at_five_minutes() {
        let records = [recorded(7, "2026-01-01T00:00:00Z")];
        let submitted = records[0].submitted_at.timestamp();
        assert!(matching(&records, 7, Some(submitted + 300)).is_some());
        assert!(matching(&records, 7, Some(submitted + 301)).is_none());
        // A clock behind recto's puts the creation before the submission.
        assert!(matching(&records, 7, Some(submitted - 60)).is_some());
        assert!(matching(&records, 7, Some(submitted - 301)).is_none());
    }

    #[tokio::test]
    async fn a_record_without_a_cups_id_matches_nothing() {
        let database = crate::testing::database().await;
        let alice = user(&database, "alice").await;
        crate::ledger::insert(
            &database,
            crate::ledger::NewEntry {
                user_id: alice.id,
                username: "alice".to_owned(),
                printer: "office".to_owned(),
                document_name: "report.pdf".to_owned(),
                document_format: "application/pdf".to_owned(),
                document_pages: None,
                copies: 1,
                sides: "one-sided".to_owned(),
                color_mode: "monochrome".to_owned(),
                counts: crate::ledger::counts(None, 1, false, false, Default::default()),
            },
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(for_cups_ids(&database, &[0, 1]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_user_with_recorded_jobs_cannot_be_deleted() {
        let database = crate::testing::database().await;
        let alice = user(&database, "alice").await;
        record(&database, alice.id, 7).await;

        let deleted = users::Entity::delete_by_id(alice.id).exec(&database).await;
        assert!(deleted.is_err());
        assert_eq!(for_cups_ids(&database, &[7]).await.unwrap().len(), 1);
    }
}
