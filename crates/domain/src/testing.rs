//! Helpers for tests: a fresh, migrated database per test, and temporary directories.
//!
//! Tests use a private in-memory SQLite database by default. Set `RECTO_TEST_DATABASE_URL` to
//! run them against another database such as PostgreSQL; the schema is then dropped and recreated
//! for each test, so run them one at a time (`--test-threads 1`, as `mise run test-postgres`
//! does).
//!
//! The module is compiled for this crate's tests and, through the `testing` feature, for the
//! server's; it is not part of the production build.

use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use recto_migration::{Migrator, MigratorTrait};
use sea_orm::DatabaseConnection;
pub use tempfile::TempDir;

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

/// A new directory under `target/test-tmp` of the workspace, never the system temporary
/// directory, removed when dropped.
pub fn temporary_directory() -> TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
    std::fs::create_dir_all(&base).expect("create target/test-tmp");
    TempDir::new_in(base).expect("create a temporary directory")
}

/// Enters a job in the ledger as if CUPS had accepted it under `cups_job_id`, with unknown
/// counts. For the tests of what reads the records.
pub async fn recorded_job(
    database: &DatabaseConnection,
    user_id: i64,
    printer: &str,
    cups_job_id: i32,
    document_name: &str,
) {
    use crate::ledger::{self, NewEntry, Prices};

    let id = ledger::insert(
        database,
        NewEntry {
            user_id,
            username: String::new(),
            printer: printer.to_owned(),
            document_name: document_name.to_owned(),
            document_format: "application/pdf".to_owned(),
            document_pages: None,
            copies: 1,
            sides: "one-sided".to_owned(),
            color_mode: "monochrome".to_owned(),
            counts: ledger::counts(None, 1, false, false, Prices::default()),
        },
        chrono::Utc::now(),
    )
    .await
    .expect("insert the ledger entry");
    ledger::attach(database, id, cups_job_id)
        .await
        .expect("attach the CUPS job id");
}
