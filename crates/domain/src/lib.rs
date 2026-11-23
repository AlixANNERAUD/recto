//! Domain of recto: the accounts (users and groups) and the database that keeps them.
//!
//! [`database`] opens, migrates and backs up the database; [`users`] and [`groups`] hold the
//! account rules, which sign-in and the administration pages share; [`access`] limits devices
//! to groups; [`jobs`] records who submitted each print job, [`ledger`] what each counts for,
//! [`prices`] what printers charge and [`quotas`] the limits on printing; [`scans`] logs every
//! finished scan and [`reports`] sums both over a range; [`device_passwords`] are what computers
//! and phones print and scan with, and [`held`] keeps what they send until it is released; the
//! `testing` module (behind the `testing` feature) gives tests a fresh, migrated database.

pub mod access;
pub mod database;
pub mod device_passwords;
pub mod error;
pub mod groups;
pub mod held;
pub mod jobs;
pub mod ledger;
pub mod names;
pub mod passwords;
pub mod prices;
pub mod quotas;
pub mod reports;
pub mod scans;
pub mod users;

pub use error::{Error, Problem, Result};

#[cfg(any(test, feature = "testing"))]
pub mod testing;
