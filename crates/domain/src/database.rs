//! Opening the database, applying migrations and taking backups.
//!
//! SQLite (the default: one file, no server) and PostgreSQL are both supported. Queries are
//! written with SeaORM's builder rather than the SQL of one database, and CI runs the test suite
//! on both (see the `testing` module); [`backup`] is the one SQLite-only operation.

use std::{path::Path, time::Duration};

use recto_migration::{Migrator, MigratorTrait};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, DbErr,
    SqliteTransactionMode, Statement, TransactionOptions, TransactionTrait,
};

/// How long a SQLite connection waits for the write lock, held by another connection, before
/// failing with `SQLITE_BUSY` ("database is locked"). Writes are short, so a wait this long
/// only happens if something is stuck.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens a connection pool; the URL scheme picks the backend, SQLite (`sqlite://…`) or
/// PostgreSQL (`postgres://…`). SQLite databases are created if missing, use write-ahead logging
/// so readers never block the writer, and enforce foreign keys (deleting a user or a group
/// relies on cascades); a PostgreSQL database must exist already. The schema is not touched:
/// call [`migrate`] afterwards.
///
/// SQLite has one writer at a time: a connection that needs the write lock waits for it up to
/// [`BUSY_TIMEOUT`]. That wait only covers statements that start a write; transactions that
/// write must start with [`begin_write`].
pub async fn connect(url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(url);
    options
        .max_connections(8)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .sqlx_logging(false)
        .map_sqlx_sqlite_opts(|options| {
            options
                .create_if_missing(true)
                .foreign_keys(true)
                .busy_timeout(BUSY_TIMEOUT)
                .pragma("journal_mode", "WAL")
                .pragma("synchronous", "NORMAL")
        });
    Database::connect(options).await
}

/// Starts a transaction that will write. On SQLite it begins with `BEGIN IMMEDIATE`, which takes
/// the write lock up front, waiting up to [`BUSY_TIMEOUT`] for it. A plain (deferred) `BEGIN`
/// would only take it at the first write, and under write-ahead logging, if another connection
/// committed since the transaction's first read, that upgrade fails at once with
/// `SQLITE_BUSY_SNAPSHOT`, which the busy timeout does not cover: the transaction read a snapshot
/// that is no longer the latest, and waiting would not help.
///
/// PostgreSQL keeps its default (`READ COMMITTED`), where writers wait for row locks. Inside a
/// transaction, this starts a savepoint, which takes no lock of its own.
pub async fn begin_write<C>(database: &C) -> Result<C::Transaction, DbErr>
where
    C: TransactionTrait + ?Sized,
{
    database
        .begin_with_options(TransactionOptions {
            sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
            ..TransactionOptions::default()
        })
        .await
}

/// Applies every pending migration of [`recto_migration::Migrator`]. Migrations only move
/// forward; there is no automatic downgrade.
pub async fn migrate(database: &DatabaseConnection) -> Result<(), DbErr> {
    Migrator::up(database, None).await
}

/// Why [`backup`] refused or failed.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error(
        "recto backup copies SQLite databases only; back up this {0:?} database with its own \
         tools, pg_dump for PostgreSQL"
    )]
    UnsupportedBackend(DatabaseBackend),
    #[error("the backup destination {0} already exists")]
    DestinationExists(String),
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// Writes a consistent copy of a live SQLite database to `destination` with `VACUUM INTO`, which
/// runs inside a read transaction and does not block other connections.
///
/// The destination must not exist (`VACUUM INTO` would fail on a non-empty file, and refusing
/// up front avoids overwriting an earlier backup). PostgreSQL databases are refused with
/// [`BackupError::UnsupportedBackend`]: use `pg_dump`.
pub async fn backup(database: &DatabaseConnection, destination: &Path) -> Result<(), BackupError> {
    let backend = database.get_database_backend();
    if backend != DatabaseBackend::Sqlite {
        return Err(BackupError::UnsupportedBackend(backend));
    }
    if destination.exists() {
        return Err(BackupError::DestinationExists(
            destination.display().to_string(),
        ));
    }
    database
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "VACUUM INTO ?",
            [destination.to_string_lossy().into_owned().into()],
        ))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use recto_entity::entities::{group_memberships, groups, sessions, user_identities, users};
    use sea_orm::{ActiveModelTrait, EntityTrait, ModelTrait, PaginatorTrait, Set};

    use super::*;

    /// Under the target directory, never the system temporary directory.
    fn temporary_directory() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        std::fs::create_dir_all(&base).expect("create target/test-tmp");
        tempfile::TempDir::new_in(base).expect("create a temporary directory")
    }

    /// A migrated SQLite file. Backups need one: SQLite opens the `VACUUM INTO` destination with
    /// the flags of the source, so copying the in-memory test databases would write in memory.
    async fn sqlite_file(directory: &tempfile::TempDir) -> DatabaseConnection {
        let path = directory.path().join("source.db");
        let database = connect(&format!("sqlite://{}", path.display()))
            .await
            .expect("open the source database");
        migrate(&database).await.expect("migrate the source");
        database
    }

    #[tokio::test]
    async fn migrations_create_every_table() {
        let database = crate::testing::database().await;

        assert!(
            users::Entity::find()
                .all(&database)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            user_identities::Entity::find()
                .all(&database)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            groups::Entity::find()
                .all(&database)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            group_memberships::Entity::find()
                .all(&database)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            sessions::Entity::find()
                .all(&database)
                .await
                .unwrap()
                .is_empty()
        );
    }

    async fn insert_user(database: &DatabaseConnection, username: &str) -> users::Model {
        users::ActiveModel {
            username: Set(username.to_owned()),
            display_name: Set(username.to_owned()),
            created_at: Set(chrono::Utc::now().fixed_offset()),
            ..Default::default()
        }
        .insert(database)
        .await
        .expect("insert a user")
    }

    async fn insert_group(database: &DatabaseConnection, name: &str) -> groups::Model {
        groups::ActiveModel {
            name: Set(name.to_owned()),
            created_at: Set(chrono::Utc::now().fixed_offset()),
            ..Default::default()
        }
        .insert(database)
        .await
        .expect("insert a group")
    }

    async fn insert_membership(database: &DatabaseConnection, group_id: i64, user_id: i64) {
        group_memberships::ActiveModel {
            group_id: Set(group_id),
            user_id: Set(user_id),
            ..Default::default()
        }
        .insert(database)
        .await
        .expect("insert a membership");
    }

    // Removing users and groups relies on these cascades; on SQLite they also prove that
    // `connect` turned foreign keys on.
    #[tokio::test]
    async fn deleting_a_user_or_a_group_removes_what_hangs_off_it() {
        let database = crate::testing::database().await;
        let alice = insert_user(&database, "alice").await;
        let bob = insert_user(&database, "bob").await;
        user_identities::ActiveModel {
            user_id: Set(alice.id),
            issuer: Set("https://id.example".to_owned()),
            subject: Set("alice".to_owned()),
            created_at: Set(chrono::Utc::now().fixed_offset()),
            ..Default::default()
        }
        .insert(&database)
        .await
        .expect("insert an identity");
        let family = insert_group(&database, "family").await;
        let friends = insert_group(&database, "friends").await;
        insert_membership(&database, family.id, alice.id).await;
        insert_membership(&database, family.id, bob.id).await;
        insert_membership(&database, friends.id, bob.id).await;

        alice.delete(&database).await.expect("delete alice");

        assert_eq!(
            user_identities::Entity::find()
                .count(&database)
                .await
                .unwrap(),
            0
        );
        let memberships = group_memberships::Entity::find()
            .all(&database)
            .await
            .unwrap();
        assert_eq!(memberships.len(), 2);
        assert!(
            memberships
                .iter()
                .all(|membership| membership.user_id == bob.id)
        );

        family
            .delete(&database)
            .await
            .expect("delete the family group");

        let memberships = group_memberships::Entity::find()
            .all(&database)
            .await
            .unwrap();
        assert_eq!(memberships.len(), 1);
        assert_eq!(memberships[0].group_id, friends.id);
        assert_eq!(users::Entity::find().count(&database).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn backup_writes_a_readable_copy() {
        let directory = temporary_directory();
        let database = sqlite_file(&directory).await;
        let destination = directory.path().join("backup.db");

        backup(&database, &destination).await.unwrap();

        let copy = connect(&format!("sqlite://{}", destination.display()))
            .await
            .unwrap();
        assert!(users::Entity::find().all(&copy).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn backup_refuses_an_existing_file() {
        let directory = temporary_directory();
        let database = sqlite_file(&directory).await;
        let destination = directory.path().join("backup.db");
        std::fs::write(&destination, "an earlier backup").unwrap();

        let error = backup(&database, &destination).await.unwrap_err();

        assert!(matches!(error, BackupError::DestinationExists(_)));
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "an earlier backup"
        );
    }

    #[tokio::test]
    async fn backup_refuses_databases_other_than_sqlite() {
        let database = crate::testing::database().await;
        // Only meaningful when the suite runs on another database (RECTO_TEST_DATABASE_URL).
        if database.get_database_backend() == DatabaseBackend::Sqlite {
            return;
        }
        let directory = temporary_directory();
        let destination = directory.path().join("backup.db");

        let error = backup(&database, &destination).await.unwrap_err();

        assert!(matches!(error, BackupError::UnsupportedBackend(_)));
        assert!(error.to_string().contains("pg_dump"));
        assert!(!destination.exists());
    }
}
