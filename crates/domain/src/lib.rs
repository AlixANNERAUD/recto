//! Domain of recto: the accounts (users and groups) and the database that keeps them.
//!
//! [`database`] opens, migrates and backs up the database; the `testing` module (behind the
//! `testing` feature) gives tests a fresh, migrated database.

pub mod database;
pub mod error;

pub use error::{Error, Result};

#[cfg(any(test, feature = "testing"))]
pub mod testing;
