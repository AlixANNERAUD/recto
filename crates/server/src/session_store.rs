//! A tower-sessions store on top of SeaORM, so sessions live in the accounts database, share its
//! connection pool and survive restarts. Ported from nanobom's `DatabaseSessionStore`.
//!
//! Records are stored as JSON with their expiry. Expired records are never returned by
//! [`SessionStore::load`] even before the periodic [`ExpiredDeletion::delete_expired`] removes
//! them, so a slow cleanup cannot extend a session.

use chrono::{DateTime, Utc};
use recto_entity::sessions;
use sea_orm::{
    ActiveValue::Set,
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    sea_query::{Expr, OnConflict},
};
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, ExpiredDeletion, SessionStore};

/// A [`SessionStore`] backed by the `sessions` table. Cloning shares the connection pool.
#[derive(Debug, Clone)]
pub struct DatabaseSessionStore {
    database: DatabaseConnection,
}

impl DatabaseSessionStore {
    pub fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }
}

fn backend(error: impl std::fmt::Display) -> session_store::Error {
    session_store::Error::Backend(error.to_string())
}

fn encode(record: &Record) -> session_store::Result<String> {
    serde_json::to_string(record).map_err(|error| session_store::Error::Encode(error.to_string()))
}

fn expiry(record: &Record) -> session_store::Result<DateTime<Utc>> {
    DateTime::from_timestamp(
        record.expiry_date.unix_timestamp(),
        record.expiry_date.nanosecond(),
    )
    .ok_or_else(|| session_store::Error::Encode("expiry date out of range".to_owned()))
}

#[async_trait::async_trait]
impl SessionStore for DatabaseSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        // Identifiers are random; on the unlikely collision, draw a new one.
        loop {
            let model = sessions::ActiveModel {
                id: Set(record.id.to_string()),
                data: Set(encode(record)?),
                expiry_date: Set(expiry(record)?.fixed_offset()),
            };
            let inserted = sessions::Entity::insert(model)
                .on_conflict(
                    OnConflict::column(sessions::Column::Id)
                        .do_nothing()
                        .to_owned(),
                )
                .exec_without_returning(&self.database)
                .await
                .map_err(backend)?;
            if inserted > 0 {
                return Ok(());
            }
            record.id = Id::default();
        }
    }

    /// Updates a record that still exists, and never brings one back, unlike nanobom's upsert.
    /// Every request saves its session, so a request that loaded it before a sign-out (or
    /// before sign-in retired its id) would otherwise write the old user back when it ends.
    /// New and cycled ids go through [`create`](SessionStore::create).
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        sessions::Entity::update_many()
            .col_expr(sessions::Column::Data, Expr::value(encode(record)?))
            .col_expr(
                sessions::Column::ExpiryDate,
                Expr::value(expiry(record)?.fixed_offset()),
            )
            .filter(sessions::Column::Id.eq(record.id.to_string()))
            .exec(&self.database)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn load(&self, id: &Id) -> session_store::Result<Option<Record>> {
        let Some(model) = sessions::Entity::find_by_id(id.to_string())
            .one(&self.database)
            .await
            .map_err(backend)?
        else {
            return Ok(None);
        };
        if model.expiry_date <= Utc::now() {
            return Ok(None);
        }
        serde_json::from_str(&model.data)
            .map(Some)
            .map_err(|error| session_store::Error::Decode(error.to_string()))
    }

    async fn delete(&self, id: &Id) -> session_store::Result<()> {
        sessions::Entity::delete_by_id(id.to_string())
            .exec(&self.database)
            .await
            .map_err(backend)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ExpiredDeletion for DatabaseSessionStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        sessions::Entity::delete_many()
            .filter(sessions::Column::ExpiryDate.lte(Utc::now().fixed_offset()))
            .exec(&self.database)
            .await
            .map_err(backend)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use recto_entity::sessions;
    use sea_orm::{DatabaseConnection, EntityTrait, PaginatorTrait};
    use tower_sessions::{
        Session, SessionStore,
        cookie::time::{Duration, OffsetDateTime},
        session::{Id, Record},
        session_store::ExpiredDeletion,
    };

    use super::*;
    use crate::session::{sign_in, sign_out};

    async fn store() -> (DatabaseSessionStore, DatabaseConnection) {
        let database = recto_domain::testing::database().await;
        (DatabaseSessionStore::new(database.clone()), database)
    }

    async fn count(database: &DatabaseConnection) -> u64 {
        sessions::Entity::find().count(database).await.unwrap()
    }

    fn session(store: &DatabaseSessionStore, id: Option<Id>) -> Session {
        Session::new(id, Arc::new(store.clone()), None)
    }

    fn record(expiry_date: OffsetDateTime) -> Record {
        Record {
            id: Id::default(),
            data: Default::default(),
            expiry_date,
        }
    }

    #[tokio::test]
    async fn records_round_trip() {
        let (store, _database) = store().await;
        let mut created = record(OffsetDateTime::now_utc() + Duration::minutes(5));
        created
            .data
            .insert("next".into(), serde_json::json!("/jobs"));
        store.create(&mut created).await.unwrap();

        let loaded = store.load(&created.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, created.id);
        assert_eq!(loaded.data, created.data);
        assert_eq!(loaded.expiry_date, created.expiry_date);

        store.delete(&created.id).await.unwrap();
        assert!(store.load(&created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_records_are_deleted() {
        let (store, database) = store().await;
        let now = OffsetDateTime::now_utc();
        let mut expired = record(now - Duration::seconds(1));
        let mut active = record(now + Duration::minutes(5));
        store.create(&mut expired).await.unwrap();
        store.create(&mut active).await.unwrap();
        // Expired records are never served, even before they are deleted.
        assert!(store.load(&expired.id).await.unwrap().is_none());
        assert_eq!(count(&database).await, 2);

        store.delete_expired().await.unwrap();
        assert_eq!(count(&database).await, 1);
        let kept = store.load(&active.id).await.unwrap().unwrap();
        assert_eq!(kept.id, active.id);
    }

    #[tokio::test]
    async fn a_stale_request_cannot_undo_a_sign_out() {
        let (store, database) = store().await;
        let signed_in = session(&store, None);
        sign_in(&signed_in, 1, "token").await.unwrap();
        signed_in.save().await.unwrap();
        let id = signed_in.id().unwrap();

        // A slow request loads the session, a sign-out ends it, then the slow request ends.
        let slow = session(&store, Some(id));
        assert_eq!(slow.get::<i64>("user_id").await.unwrap(), Some(1));
        assert_eq!(
            sign_out(&session(&store, Some(id))).await.as_deref(),
            Some("token")
        );
        slow.save().await.unwrap();

        assert!(store.load(&id).await.unwrap().is_none());
        assert_eq!(count(&database).await, 0);
    }

    #[tokio::test]
    async fn a_stale_request_cannot_revive_an_id_retired_at_sign_in() {
        let (store, database) = store().await;
        let pending = session(&store, None);
        pending.insert("openid.next", "/jobs").await.unwrap();
        pending.save().await.unwrap();
        let old = pending.id().unwrap();

        let slow = session(&store, Some(old));
        assert!(slow.get::<String>("openid.next").await.unwrap().is_some());
        let callback = session(&store, Some(old));
        sign_in(&callback, 1, "token").await.unwrap();
        callback.save().await.unwrap();
        let new = callback.id().unwrap();
        assert_ne!(new, old);
        slow.save().await.unwrap();

        assert!(store.load(&old).await.unwrap().is_none());
        assert!(store.load(&new).await.unwrap().is_some());
        assert_eq!(count(&database).await, 1);
    }
}
