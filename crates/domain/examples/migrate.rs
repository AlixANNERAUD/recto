//! Applies the migrations to the database at the URL given as the only argument, creating a
//! SQLite file if missing. `mise run entities` runs it on a throwaway database to generate the
//! entities from.

use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Some(url) = std::env::args().nth(1) else {
        eprintln!("usage: migrate <database-url>");
        return ExitCode::FAILURE;
    };
    let result = async {
        let database = recto_domain::database::connect(&url).await?;
        recto_domain::database::migrate(&database).await
    }
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The URL is left out: a PostgreSQL URL can hold a password.
            eprintln!("migrating the database failed: {error}");
            ExitCode::FAILURE
        }
    }
}
