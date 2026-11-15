//! The error type of domain operations.

use sea_orm::DbErr;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database failed. The message may reveal schema details, so it is logged and never
    /// shown to the user.
    #[error("database error: {0}")]
    Database(#[from] DbErr),
}

/// The result of a domain operation.
pub type Result<T, E = Error> = std::result::Result<T, E>;
