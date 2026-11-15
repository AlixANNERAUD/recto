//! The administration commands, called as functions on a test database with the password passed
//! directly instead of read from a terminal.

use recto::{
    commands,
    config::{AdministratorOptions, CreateAdministratorOptions, DatabaseOptions},
};
use recto_domain::{
    passwords,
    testing::{self, temporary_directory},
    users,
};

fn database_options() -> DatabaseOptions {
    DatabaseOptions {
        database_url: "sqlite::memory:".to_owned(),
    }
}

fn create_options(username: &str) -> CreateAdministratorOptions {
    CreateAdministratorOptions {
        username: username.to_owned(),
        display_name: None,
        email: Some("admin@example.org".to_owned()),
        password_minimum_length: 12,
        database: database_options(),
    }
}

fn administrator_options(username: &str, remove: bool) -> AdministratorOptions {
    AdministratorOptions {
        username: username.to_owned(),
        remove,
        database: database_options(),
    }
}

#[tokio::test]
async fn create_administrator_makes_a_local_admin() {
    let database = testing::database().await;
    commands::create_administrator(&database, &create_options("root"), || {
        Ok("correct horse battery".to_owned())
    })
    .await
    .unwrap();
    let user = users::find_by_username(&database, "root")
        .await
        .unwrap()
        .unwrap();
    assert!(user.is_administrator);
    assert_eq!(user.display_name, "root");
    assert!(
        passwords::verify(
            "correct horse battery".to_owned(),
            user.password_hash.clone().unwrap()
        )
        .await
    );
}

#[tokio::test]
async fn create_administrator_refuses_a_short_password() {
    let database = testing::database().await;
    let error = commands::create_administrator(&database, &create_options("root"), || {
        Ok("short".to_owned())
    })
    .await
    .unwrap_err();
    assert!(error.contains("at least 12 characters"), "{error}");
    assert!(
        users::find_by_username(&database, "root")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn administrator_grants_and_removes() {
    let database = testing::database().await;
    commands::create_administrator(&database, &create_options("root"), || {
        Ok("correct horse battery".to_owned())
    })
    .await
    .unwrap();
    // The last administrator can lose the rights from the command line.
    commands::administrator(&database, &administrator_options("root", true))
        .await
        .unwrap();
    let user = users::find_by_username(&database, "root")
        .await
        .unwrap()
        .unwrap();
    assert!(!user.is_administrator);
    commands::administrator(&database, &administrator_options("root", false))
        .await
        .unwrap();
    let user = users::find_by_username(&database, "root")
        .await
        .unwrap()
        .unwrap();
    assert!(user.is_administrator);
    assert!(!user.administrator_from_claim);
}

#[tokio::test]
async fn administrator_reports_an_unknown_user() {
    let database = testing::database().await;
    let error = commands::administrator(&database, &administrator_options("ghost", false))
        .await
        .unwrap_err();
    assert_eq!(error, "no user named ghost");
}

#[test]
fn backup_command_refuses_postgres_urls() {
    for url in ["postgres://u:p@db/recto", "postgresql://db/recto"] {
        let error = commands::check_backup_url(url).unwrap_err();
        assert!(error.contains("pg_dump"), "{error}");
    }
    assert!(commands::check_backup_url("sqlite:///data/recto.db").is_ok());
}

#[tokio::test]
async fn backup_copies_a_migrated_database() {
    let directory = temporary_directory();
    let source = directory.path().join("source.db");
    let url = format!("sqlite://{}", source.display());
    commands::migrate(&url).await.unwrap();
    let destination = directory.path().join("copy.db");
    commands::backup(&url, &destination).await.unwrap();

    let copy = recto_domain::database::connect(&format!("sqlite://{}", destination.display()))
        .await
        .unwrap();
    assert!(users::list(&copy).await.unwrap().is_empty());
}

#[tokio::test]
async fn backup_refuses_an_existing_destination() {
    let directory = temporary_directory();
    let url = format!("sqlite://{}", directory.path().join("source.db").display());
    commands::migrate(&url).await.unwrap();
    let destination = directory.path().join("copy.db");
    std::fs::write(&destination, b"keep").unwrap();
    let error = commands::backup(&url, &destination).await.unwrap_err();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(std::fs::read(&destination).unwrap(), b"keep");
}

#[tokio::test]
async fn backup_refuses_a_missing_database_and_creates_nothing() {
    let directory = temporary_directory();
    let missing = directory.path().join("typo.db");
    let url = format!("sqlite://{}", missing.display());
    let error = commands::backup(&url, &directory.path().join("copy.db"))
        .await
        .unwrap_err();
    assert!(error.contains("does not exist"), "{error}");
    assert!(error.contains("typo.db"), "{error}");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    assert!(commands::require_existing_database(&url).is_err());
}
