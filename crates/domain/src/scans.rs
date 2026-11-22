//! The scan log: one row of `scan_jobs` per finished scan, so the usage reports can say who
//! scanned what and when. Previews are not logged, and recto never keeps the content.
//!
//! The log is best effort: a scan that could not be logged is still handed over, since scans
//! have no quotas to enforce.

use chrono::{DateTime, Utc};
use recto_entity::scan_jobs;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
};

use crate::error::Result;

/// Where a scan came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The scan page.
    Web,
    /// A computer or phone on the network, through eSCL.
    Escl,
}

impl Source {
    /// The keyword stored in `scan_jobs.source`.
    pub fn keyword(self) -> &'static str {
        match self {
            Source::Web => "web",
            Source::Escl => "escl",
        }
    }
}

/// A scan to log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewScan {
    /// `None` for an anonymous network scan.
    pub user_id: Option<i64>,
    /// Copied, so the history keeps the name the user had; empty when anonymous.
    pub username: String,
    /// The SANE device id.
    pub scanner: String,
    /// The stored result's UUID, when there is one.
    pub result_id: Option<String>,
    pub source: Source,
    /// The file extension: `pdf`, `png` or `jpg`.
    pub format: String,
    pub pages: u32,
}

/// Logs a scan, not saved yet; returns its id.
pub async fn record(db: &impl ConnectionTrait, scan: NewScan, now: DateTime<Utc>) -> Result<i64> {
    let row = scan_jobs::ActiveModel {
        user_id: Set(scan.user_id),
        username: Set(scan.username),
        scanner: Set(scan.scanner),
        result_id: Set(scan.result_id),
        source: Set(scan.source.keyword().to_owned()),
        format: Set(scan.format),
        pages: Set(i64::from(scan.pages)),
        saved: Set(false),
        scanned_at: Set(now.fixed_offset()),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(row.id)
}

/// Marks the user's scan saved; false when no row matches (a scan logged before this table, or
/// whose log failed).
pub async fn mark_saved(db: &impl ConnectionTrait, user_id: i64, result_id: &str) -> Result<bool> {
    let result = scan_jobs::Entity::update_many()
        .col_expr(scan_jobs::Column::Saved, true.into())
        .filter(scan_jobs::Column::UserId.eq(user_id))
        .filter(scan_jobs::Column::ResultId.eq(result_id))
        .exec(db)
        .await?;
    Ok(result.rows_affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::{Identity, sign_in_with_identity};

    async fn person(db: &sea_orm::DatabaseConnection, name: &str) -> i64 {
        let identity = Identity {
            issuer: "https://id.example".to_owned(),
            subject: name.to_owned(),
            preferred_username: Some(name.to_owned()),
            ..Identity::default()
        };
        sign_in_with_identity(db, &identity).await.unwrap().id
    }

    async fn row(db: &sea_orm::DatabaseConnection, id: i64) -> scan_jobs::Model {
        scan_jobs::Entity::find_by_id(id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn a_scan_is_recorded_and_marked_saved() {
        let db = crate::testing::database().await;
        let alice = person(&db, "alice").await;
        let bob = person(&db, "bob").await;
        let now: DateTime<Utc> = "2026-11-22T10:00:00Z".parse().unwrap();
        let result = "0b6f4c2e-6a39-4d55-9f43-b8d3c1d0a001";

        let id = record(
            &db,
            NewScan {
                user_id: Some(alice),
                username: "alice".to_owned(),
                scanner: "mock:0".to_owned(),
                result_id: Some(result.to_owned()),
                source: Source::Web,
                format: "pdf".to_owned(),
                pages: 3,
            },
            now,
        )
        .await
        .unwrap();
        let scan = row(&db, id).await;
        assert_eq!(scan.user_id, Some(alice));
        assert_eq!(scan.username, "alice");
        assert_eq!(scan.scanner, "mock:0");
        assert_eq!(scan.result_id.as_deref(), Some(result));
        assert_eq!(scan.source, "web");
        assert_eq!(scan.format, "pdf");
        assert_eq!(scan.pages, 3);
        assert!(!scan.saved);
        assert_eq!(scan.scanned_at.with_timezone(&Utc), now);

        // A network scan has no user and no stored result; several such rows coexist.
        for _ in 0..2 {
            let anonymous = record(
                &db,
                NewScan {
                    user_id: None,
                    username: String::new(),
                    scanner: "mock:0".to_owned(),
                    result_id: None,
                    source: Source::Escl,
                    format: "jpg".to_owned(),
                    pages: 1,
                },
                now,
            )
            .await
            .unwrap();
            let anonymous = row(&db, anonymous).await;
            assert_eq!(anonymous.user_id, None);
            assert_eq!(anonymous.source, "escl");
        }

        assert!(!mark_saved(&db, bob, result).await.unwrap());
        assert!(!row(&db, id).await.saved);
        assert!(mark_saved(&db, alice, result).await.unwrap());
        assert!(row(&db, id).await.saved);
        // Saving twice is still a match.
        assert!(mark_saved(&db, alice, result).await.unwrap());
        assert!(
            !mark_saved(&db, alice, "ffffffff-ffff-ffff-ffff-ffffffffffff")
                .await
                .unwrap()
        );
    }
}
