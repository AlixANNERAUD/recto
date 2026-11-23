//! Drops `index_print_jobs_user`: `index_print_jobs_user_submitted_at` starts with the same
//! column, so it serves every lookup by user, the foreign key's included, and the narrower index
//! only cost a write on each job.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum PrintJobs {
    Table,
    UserId,
}

const INDEX: &str = "index_print_jobs_user";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name(INDEX)
                    .table(PrintJobs::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name(INDEX)
                    .table(PrintJobs::Table)
                    .col(PrintJobs::UserId)
                    .to_owned(),
            )
            .await
    }
}
