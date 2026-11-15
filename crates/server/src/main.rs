//! The recto binary: parses the command line, sets up logging and hands over to the library.

use std::process::ExitCode;

use clap::Parser;
use recto::{
    commands,
    config::{Command, CommandLine, Config, DatabaseOptions, LogFormat},
    serve,
    state::open_database,
};

#[tokio::main]
async fn main() -> ExitCode {
    let command_line = CommandLine::parse();
    let result: Result<(), Box<dyn std::error::Error>> = match command_line.command {
        Command::Serve(config) => {
            let config = Config::normalized(*config).unwrap_or_else(|error| error.exit());
            initialize_logging(config.log_format);
            serve::run(config).await
        }
        Command::Healthcheck(options) => serve::healthcheck(options.listen_address).await,
        Command::Migrate(options) => commands::migrate(&options.database_url)
            .await
            .map_err(Into::into),
        Command::Backup(options) => commands::backup(&options.database.database_url, &options.file)
            .await
            .map_err(Into::into),
        Command::CreateAdministrator(options) => {
            // The database is opened first so that a bad URL fails before any password prompt.
            let database = open_for_command(&options.database).await;
            match database {
                Ok(database) => {
                    commands::create_administrator(&database, &options, commands::read_password)
                        .await
                        .map_err(Into::into)
                }
                Err(error) => Err(error.into()),
            }
        }
        Command::Administrator(options) => {
            // Never start a database here: a wrong path would get an empty one and report that
            // the user does not exist.
            match commands::require_existing_database(&options.database.database_url) {
                Err(error) => Err(error.into()),
                Ok(()) => match open_for_command(&options.database).await {
                    Ok(database) => commands::administrator(&database, &options)
                        .await
                        .map_err(Into::into),
                    Err(error) => Err(error.into()),
                },
            }
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("recto: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Opens the database for a command that changes it; the messages of `tracing` (a created user)
/// go to standard error as text.
async fn open_for_command(
    options: &DatabaseOptions,
) -> Result<sea_orm::DatabaseConnection, String> {
    initialize_logging(LogFormat::Text);
    open_database(&options.database_url).await
}

/// Logs go to standard error: JSON lines by default, text for development.
fn initialize_logging(format: LogFormat) {
    let builder = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Json => builder.json().flatten_event(true).init(),
        LogFormat::Text => builder.init(),
    }
}
