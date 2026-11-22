//! The scan log, and an index for the usage reports.
//!
//! `scan_jobs` keeps one row per finished scan (never previews): who scanned, on which scanner,
//! how many pages, in which format, and whether the file was saved to the output folder. Like
//! the print ledger it is history: the username is a snapshot and a user with scans cannot be
//! deleted.
//!
//! The user and the stored result are optional, so that scans from the network (eSCL), which may
//! have neither, fit the same table; `source` says where a scan came from.
//!
//! The usage reports sum the ledger over a range for every user, which the index on
//! (`user_id`, `submitted_at`) cannot serve: `print_jobs` gets one on `submitted_at` alone.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum PrintJobs {
    Table,
    SubmittedAt,
}

#[derive(DeriveIden)]
enum ScanJobs {
    Table,
    Id,
    UserId,
    Username,
    Scanner,
    ResultId,
    Source,
    Format,
    Pages,
    Saved,
    ScannedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ScanJobs::Table)
                    .col(big_pk_auto(ScanJobs::Id))
                    .col(big_integer_null(ScanJobs::UserId))
                    // A snapshot, empty for an anonymous scan.
                    .col(string(ScanJobs::Username))
                    // The SANE device id, the name access rules use.
                    .col(string(ScanJobs::Scanner))
                    // The stored result's UUID, which Save finds the row by.
                    .col(string_null(ScanJobs::ResultId))
                    // `web` or `escl`.
                    .col(string(ScanJobs::Source).default("web").to_owned())
                    // The file extension: `pdf`, `png` or `jpg`.
                    .col(string(ScanJobs::Format))
                    .col(big_integer(ScanJobs::Pages))
                    .col(boolean(ScanJobs::Saved).default(false).to_owned())
                    .col(timestamp_with_time_zone(ScanJobs::ScannedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_scan_jobs_user")
                            .from(ScanJobs::Table, ScanJobs::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("index_scan_jobs_scanned_at")
                    .table(ScanJobs::Table)
                    .col(ScanJobs::ScannedAt)
                    .to_owned(),
            )
            .await?;
        // Both databases allow several nulls in a unique index.
        manager
            .create_index(
                Index::create()
                    .name("index_scan_jobs_result_id")
                    .table(ScanJobs::Table)
                    .col(ScanJobs::ResultId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("index_print_jobs_submitted_at")
                    .table(PrintJobs::Table)
                    .col(PrintJobs::SubmittedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("index_print_jobs_submitted_at")
                    .table(PrintJobs::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ScanJobs::Table).to_owned())
            .await
    }
}
