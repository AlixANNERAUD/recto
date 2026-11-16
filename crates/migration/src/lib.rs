//! Database migrations of recto, written with SeaORM's schema builder so they run on every
//! database SeaORM supports. They are embedded in the binary and applied at startup.
//!
//! To change the schema, add a migration rather than editing one that was released: databases
//! that already applied it would never see the change. Name it `mYYYYMMDD_NNNNNN_purpose`,
//! append it to [`Migrator::migrations`], then run `mise run entities` to regenerate the
//! `recto-entity` crate from the migrated schema (entities are never edited by hand).
//!
//! SQLite can only add one column per `ALTER TABLE` and cannot add foreign keys to an existing
//! table, so migrations alter tables one column at a time and new relations get new tables.

pub use sea_orm_migration::prelude::*;

mod m20261115_000001_accounts;
mod m20261116_000001_print_jobs;
mod m20261116_000002_device_access;
mod m20261117_000001_ledger;

/// Every migration of recto, in the order they apply; used by `recto_domain::database::migrate`.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261115_000001_accounts::Migration),
            Box::new(m20261116_000001_print_jobs::Migration),
            Box::new(m20261116_000002_device_access::Migration),
            Box::new(m20261117_000001_ledger::Migration),
        ]
    }
}
