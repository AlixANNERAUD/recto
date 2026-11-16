//! The jobs recto submitted: which user sent which CUPS job. CUPS only knows the name recto
//! printed in, so this is what lets the job list show real users.
//!
//! The CUPS job id may be missing, so that a record can be written before CUPS answers. A user
//! with records cannot be deleted: the records are the usage history, which must survive.

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
    Id,
    UserId,
    Printer,
    CupsJobId,
    DocumentName,
    SubmittedAt,
    State,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PrintJobs::Table)
                    .col(big_pk_auto(PrintJobs::Id))
                    .col(big_integer(PrintJobs::UserId))
                    .col(string(PrintJobs::Printer))
                    // 64 bits like SQLite's integers, so the entity reads the same on PostgreSQL.
                    .col(big_integer_null(PrintJobs::CupsJobId))
                    .col(string(PrintJobs::DocumentName))
                    .col(timestamp_with_time_zone(PrintJobs::SubmittedAt))
                    .col(string(PrintJobs::State))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_print_jobs_user")
                            .from(PrintJobs::Table, PrintJobs::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;
        // The job list looks records up by CUPS id; the index on the user also serves the
        // check that refuses to delete a user with records.
        manager
            .create_index(
                Index::create()
                    .name("index_print_jobs_cups_job_id")
                    .table(PrintJobs::Table)
                    .col(PrintJobs::CupsJobId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("index_print_jobs_user")
                    .table(PrintJobs::Table)
                    .col(PrintJobs::UserId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PrintJobs::Table).to_owned())
            .await
    }
}
