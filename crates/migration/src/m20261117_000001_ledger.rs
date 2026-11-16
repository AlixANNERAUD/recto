//! The ledger columns of `print_jobs`: who printed what, the counts computed at submission and
//! whether the job finished or was refunded. The quotas and the usage reports read them.
//!
//! The counts are null when they are unknown (a PDF recto could not read), which is not the same
//! as 0. The jobs recorded before this migration were never counted: their counts stay null, their
//! cost 0, and they are marked finished, because they are history that the job state poll must
//! not look at again.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum PrintJobs {
    Table,
    UserId,
    SubmittedAt,
    Username,
    DocumentFormat,
    DocumentPages,
    Copies,
    Sides,
    ColorMode,
    Pages,
    Sheets,
    ColorPages,
    MonoPages,
    Cost,
    Refunded,
    Finished,
    SeenProcessing,
    ImpressionsCompleted,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite adds one column per statement.
        let columns = [
            // Big integers throughout: 64 bits like SQLite, so entities read the same on PostgreSQL.
            // A snapshot: the username may change or the account go, the history stays.
            string(PrintJobs::Username).default("").to_owned(),
            string_null(PrintJobs::DocumentFormat),
            big_integer_null(PrintJobs::DocumentPages),
            big_integer(PrintJobs::Copies).default(1).to_owned(),
            string_null(PrintJobs::Sides),
            string_null(PrintJobs::ColorMode),
            big_integer_null(PrintJobs::Pages),
            big_integer_null(PrintJobs::Sheets),
            big_integer_null(PrintJobs::ColorPages),
            big_integer_null(PrintJobs::MonoPages),
            big_integer(PrintJobs::Cost).default(0).to_owned(),
            boolean(PrintJobs::Refunded).default(false).to_owned(),
            boolean(PrintJobs::Finished).default(false).to_owned(),
            boolean(PrintJobs::SeenProcessing).default(false).to_owned(),
            big_integer_null(PrintJobs::ImpressionsCompleted),
        ];
        for column in columns {
            manager
                .alter_table(
                    Table::alter()
                        .table(PrintJobs::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        // Every row so far predates the ledger, so none is waiting for a final state.
        manager
            .exec_stmt(
                Query::update()
                    .table(PrintJobs::Table)
                    .value(PrintJobs::Finished, true)
                    .to_owned(),
            )
            .await?;
        // Quota checks sum a user's jobs since the start of a period; the state poll looks for
        // the jobs that are not finished.
        manager
            .create_index(
                Index::create()
                    .name("index_print_jobs_user_submitted_at")
                    .table(PrintJobs::Table)
                    .col(PrintJobs::UserId)
                    .col(PrintJobs::SubmittedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("index_print_jobs_finished")
                    .table(PrintJobs::Table)
                    .col(PrintJobs::Finished)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for index in [
            "index_print_jobs_finished",
            "index_print_jobs_user_submitted_at",
        ] {
            manager
                .drop_index(Index::drop().name(index).table(PrintJobs::Table).to_owned())
                .await?;
        }
        for column in [
            PrintJobs::ImpressionsCompleted,
            PrintJobs::SeenProcessing,
            PrintJobs::Finished,
            PrintJobs::Refunded,
            PrintJobs::Cost,
            PrintJobs::MonoPages,
            PrintJobs::ColorPages,
            PrintJobs::Sheets,
            PrintJobs::Pages,
            PrintJobs::ColorMode,
            PrintJobs::Sides,
            PrintJobs::Copies,
            PrintJobs::DocumentPages,
            PrintJobs::DocumentFormat,
            PrintJobs::Username,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(PrintJobs::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
