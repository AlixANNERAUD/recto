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
async fn create_administrator_refuses_an_impossible_minimum_length() {
    let database = testing::database().await;
    for minimum in [0, 1025] {
        let options = CreateAdministratorOptions {
            password_minimum_length: minimum,
            ..create_options("root")
        };
        let error = commands::create_administrator(&database, &options, || {
            panic!("the password is not asked for")
        })
        .await
        .unwrap_err();
        assert!(error.contains("RECTO_PASSWORD_MINIMUM_LENGTH"), "{error}");
    }
    assert!(users::list(&database).await.unwrap().is_empty());
}

#[test]
fn a_piped_password_is_one_line() {
    assert_eq!(
        commands::read_password_line(&b"correct horse battery\r\nmore"[..]).unwrap(),
        "correct horse battery"
    );
    assert_eq!(commands::read_password_line(&b"last"[..]).unwrap(), "last");
    let error = commands::read_password_line(&b""[..]).unwrap_err();
    assert!(error.contains("no password"), "{error}");
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

#[test]
fn dnssd_services_takes_a_directory_and_the_published_port() {
    use clap::Parser as _;
    use recto::config::{Command, CommandLine};
    let parse = |extra: &[&str]| {
        let mut arguments = vec![
            "recto",
            "dnssd-services",
            "/etc/avahi/services",
            "--public-url",
            "https://recto.example.org",
        ];
        arguments.extend_from_slice(extra);
        match CommandLine::try_parse_from(arguments).unwrap().command {
            Command::DnssdServices(options) => options,
            _ => panic!("not dnssd-services"),
        }
    };
    let options = parse(&[]);
    assert_eq!(options.dir, std::path::Path::new("/etc/avahi/services"));
    assert_eq!(options.port, 8631);
    assert_eq!(parse(&["--port", "18631"]).port, 18631);
}

#[test]
fn dnssd_services_writes_avahi_files_and_removes_stale_ones() {
    use recto::dnssd::{self, AdvertisedScanner};
    let directory = temporary_directory();
    let stale = directory.path().join("recto-scanner-00000000.service");
    let unrelated = directory.path().join("printer.service");
    std::fs::write(&stale, "a scanner that is gone").unwrap();
    std::fs::write(&unrelated, "kept").unwrap();
    let public_url = url::Url::parse("https://recto.example.org").unwrap();
    let mut advertised = dnssd::printer("recto", "recto-nas.local.", 8631, &public_url);
    advertised.scanners.push(AdvertisedScanner {
        key: "1a2b3c4d".into(),
        name: "Canon LiDE 300 (recto)".into(),
        uuid: uuid::Uuid::nil(),
        make_and_model: "Canon LiDE 300".into(),
        color: true,
        grayscale: true,
        binary: false,
        platen: true,
        adf: false,
        duplex: false,
    });

    let written = commands::write_avahi_files(&advertised, directory.path()).unwrap();
    let names: Vec<String> = written
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["recto-printer.service", "recto-scanner-1a2b3c4d.service"]
    );
    for (path, (_, contents)) in written.iter().zip(dnssd::avahi_files(&advertised)) {
        assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
    }
    assert!(!stale.exists());
    assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "kept");
    // Nothing half written is left behind.
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 3);
}
