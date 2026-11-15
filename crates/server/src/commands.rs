//! The administration commands of the binary: `migrate`, `backup`, `create-administrator` and
//! `administrator`. They work on the database directly, without a signed-in user: whoever can run
//! them can read the database anyway.

use std::{io::BufRead, path::Path};

use recto_domain::{
    database::{self, BackupError},
    passwords::PasswordPolicy,
    users::{self, NewLocalUser},
};
use sea_orm::DatabaseConnection;

use crate::{
    config::{AdministratorOptions, CreateAdministratorOptions},
    state::{open_database, without_password},
};

/// Applies the pending migrations.
pub async fn migrate(url: &str) -> Result<(), String> {
    open_database(url).await.map(|_| ())
}

/// Refuses what `backup` cannot copy before touching the database, so a PostgreSQL server is not
/// even contacted.
pub fn check_backup_url(url: &str) -> Result<(), String> {
    if url.starts_with("postgres:") || url.starts_with("postgresql:") {
        return Err(
            "recto backup copies SQLite databases only; back up a PostgreSQL database with its \
             own tools, pg_dump"
                .to_owned(),
        );
    }
    Ok(())
}

/// Writes a consistent copy of the SQLite database to `destination`. It connects without
/// migrating, so a backup never changes the database it copies.
pub async fn backup(url: &str, destination: &Path) -> Result<(), String> {
    check_backup_url(url)?;
    let database = database::connect(url).await.map_err(|error| {
        format!(
            "cannot open the database {}: {error}",
            without_password(url)
        )
    })?;
    database::backup(&database, destination)
        .await
        .map_err(|error| match error {
            BackupError::Database(error) => format!("the backup failed: {error}"),
            other => other.to_string(),
        })
}

/// Reads a password on a terminal without echo, asking twice, or one line from standard input
/// otherwise so that it can be piped.
pub fn read_password() -> Result<String, String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        let read = |prompt: &str| {
            rpassword::prompt_password(prompt)
                .map_err(|error| format!("cannot read the password: {error}"))
        };
        let password = read("Password: ")?;
        if password != read("Password again: ")? {
            return Err("the passwords do not match".to_owned());
        }
        Ok(password)
    } else {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|error| format!("cannot read the password: {error}"))?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

/// Creates a local administrator. `read_password` is a parameter so tests need no terminal; it
/// runs only once the database is open, so a bad URL fails before any prompt.
pub async fn create_administrator(
    database: &DatabaseConnection,
    options: &CreateAdministratorOptions,
    read_password: impl FnOnce() -> Result<String, String>,
) -> Result<(), String> {
    let password = read_password()?;
    let user = users::create_local(
        database,
        NewLocalUser {
            username: options.username.clone(),
            display_name: options.display_name.clone().unwrap_or_default(),
            email: options.email.clone(),
            password,
            is_administrator: true,
        },
        &PasswordPolicy {
            minimum_length: options.password_minimum_length,
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    tracing::info!(user = %user.username, "administrator created");
    Ok(())
}

/// Grants or removes administration as a manual grant. The command line has no "self", so it may
/// remove any administrator, the last one included.
pub async fn administrator(
    database: &DatabaseConnection,
    options: &AdministratorOptions,
) -> Result<(), String> {
    let user = users::find_by_username(database, &options.username)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("no user named {}", options.username))?;
    users::set_administrator_from_command_line(database, user.id, !options.remove)
        .await
        .map_err(|error| error.to_string())
}
