//! Domain of recto: the accounts (users and groups) and the database that keeps them.
//!
//! [`database`] opens, migrates and backs up the database; [`users`] and [`groups`] hold the
//! account rules, which sign-in and the administration pages share; the `testing` module (behind
//! the `testing` feature) gives tests a fresh, migrated database.

pub mod database;
pub mod error;
pub mod groups;
pub mod names;
pub mod passwords;
pub mod users;

pub use error::{Error, Problem, Result};

#[cfg(any(test, feature = "testing"))]
pub mod testing;
