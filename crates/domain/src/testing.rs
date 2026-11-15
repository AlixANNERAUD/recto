//! Helpers for tests: a fresh, migrated database per test.
//!
//! Tests use a private in-memory SQLite database by default. Set `RECTO_TEST_DATABASE_URL` to
//! run them against another database such as PostgreSQL; the schema is then dropped and recreated
//! for each test, so run them one at a time (`--test-threads 1`, as `mise run test-postgres`
//! does).
//!
//! The module is compiled for this crate's tests and, through the `testing` feature, for the
//! server's; it is not part of the production build.

use std::sync::atomic::{AtomicU64, Ordering};

use recto_migration::{Migrator, MigratorTrait};
use sea_orm::DatabaseConnection;

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// Returns a connection to an empty database with every migration applied. Each call gets its
/// own SQLite in-memory database, named after the process and a counter, which lives as long as
/// a connection to it stays open.
pub async fn database() -> DatabaseConnection {
    match std::env::var("RECTO_TEST_DATABASE_URL") {
        Ok(url) => {
            let database = crate::database::connect(&url)
                .await
                .expect("connect to the test database");
            Migrator::fresh(&database)
                .await
                .expect("recreate the test schema");
            database
        }
        Err(_) => {
            let number = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
            let url = format!(
                "sqlite:file:recto-test-{}-{number}?mode=memory&cache=shared",
                std::process::id()
            );
            let database = crate::database::connect(&url)
                .await
                .expect("open the in-memory test database");
            crate::database::migrate(&database)
                .await
                .expect("migrate the test database");
            database
        }
    }
}
