//! Printing directly: the printer a user's computers and phones print to at once, instead of
//! holding their documents until the user releases them. `NULL` holds them.
//!
//! A plain name, not a foreign key: printers live in CUPS, not in the database, and a printer
//! that goes away or that the user may no longer use only makes the next job wait, held.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Users {
    Table,
    DirectPrinter,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(string_null(Users::DirectPrinter))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::DirectPrinter)
                    .to_owned(),
            )
            .await
    }
}
