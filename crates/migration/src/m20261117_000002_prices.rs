//! What each printer charges, in minor currency units. A printer without a row is free.
//!
//! Printers are named as CUPS names them, like the device access rules, so the prices outlive a
//! printer that is removed outside recto until an administrator forgets them. The ledger copies
//! the cost of each job at submission, so changing a price never changes the past.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum PrinterPrices {
    Table,
    Printer,
    MonoPage,
    ColorPage,
    Sheet,
    UpdatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PrinterPrices::Table)
                    .col(string(PrinterPrices::Printer).primary_key())
                    .col(big_integer(PrinterPrices::MonoPage).default(0))
                    .col(big_integer(PrinterPrices::ColorPage).default(0))
                    .col(big_integer(PrinterPrices::Sheet).default(0))
                    .col(timestamp_with_time_zone(PrinterPrices::UpdatedAt))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PrinterPrices::Table).to_owned())
            .await
    }
}
