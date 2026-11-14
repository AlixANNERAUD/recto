//! The recto binary: parses the command line, sets up logging and hands over to the library.

use std::process::ExitCode;

use clap::Parser;
use recto::{
    config::{CommandLine, Config, LogFormat},
    serve,
};

#[tokio::main]
async fn main() -> ExitCode {
    let command_line = CommandLine::parse();
    let result = match command_line.command {
        recto::config::Command::Serve(config) => {
            let config = Config::normalized(*config);
            initialize_logging(config.log_format);
            serve::run(config).await
        }
        recto::config::Command::Healthcheck(options) => {
            serve::healthcheck(options.listen_address).await
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("recto stopped: {error}");
            ExitCode::FAILURE
        }
    }
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
