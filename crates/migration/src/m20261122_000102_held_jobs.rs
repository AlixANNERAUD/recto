//! Held print jobs: documents computers and phones sent to recto, kept until their owner
//! releases them to a printer, deletes them, or they expire. The document itself is a file under
//! `RECTO_DATA_DIR/held`, named in `file`.
//!
//! `state` is `incoming` (an IPP Create-Job still waiting for its document), `held`,
//! `releasing` (claimed by one release) or `released` (entered in the ledger). `claim` counts the
//! claims of the row, so that a release that lost its claim to the sweep cannot mark the row
//! released. `ledger_entry_id` is set in the same transaction as the ledger insert: a row that
//! has one never goes back to `held`, so a job never prints twice.
//!
//! The sweep looks rows up by state and expiry, hence the index on (`state`, `expires_at`).

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum DevicePasswords {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum PrintJobs {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum HeldJobs {
    Table,
    Id,
    UserId,
    DevicePasswordId,
    State,
    Claim,
    Name,
    StoredFormat,
    ReceivedFormat,
    DocumentPages,
    Size,
    File,
    Requested,
    Refusal,
    LedgerEntryId,
    ReceivedAt,
    ExpiresAt,
    StateChangedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(HeldJobs::Table)
                    // Also the IPP job id, which must stay below 2^31.
                    .col(big_pk_auto(HeldJobs::Id))
                    .col(big_integer(HeldJobs::UserId))
                    // Revoking the password keeps the job: its owner may still release it.
                    .col(big_integer_null(HeldJobs::DevicePasswordId))
                    .col(string(HeldJobs::State))
                    .col(big_integer(HeldJobs::Claim).default(0).to_owned())
                    .col(string(HeldJobs::Name))
                    // The MIME type of the file, once there is one.
                    .col(string_null(HeldJobs::StoredFormat))
                    // The MIME type the device sent, before any conversion.
                    .col(string_null(HeldJobs::ReceivedFormat))
                    .col(big_integer_null(HeldJobs::DocumentPages))
                    .col(big_integer(HeldJobs::Size).default(0).to_owned())
                    .col(string_null(HeldJobs::File))
                    // The options the device asked for, as JSON.
                    .col(text(HeldJobs::Requested))
                    // Why the last release was refused, as a refusal code.
                    .col(string_null(HeldJobs::Refusal))
                    .col(big_integer_null(HeldJobs::LedgerEntryId))
                    .col(timestamp_with_time_zone(HeldJobs::ReceivedAt))
                    .col(timestamp_with_time_zone(HeldJobs::ExpiresAt))
                    .col(timestamp_with_time_zone(HeldJobs::StateChangedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_held_jobs_user")
                            .from(HeldJobs::Table, HeldJobs::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_held_jobs_device_password")
                            .from(HeldJobs::Table, HeldJobs::DevicePasswordId)
                            .to(DevicePasswords::Table, DevicePasswords::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_held_jobs_ledger_entry")
                            .from(HeldJobs::Table, HeldJobs::LedgerEntryId)
                            .to(PrintJobs::Table, PrintJobs::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        for (name, column) in [
            ("index_held_jobs_user", HeldJobs::UserId),
            (
                "index_held_jobs_device_password",
                HeldJobs::DevicePasswordId,
            ),
        ] {
            manager
                .create_index(
                    Index::create()
                        .name(name)
                        .table(HeldJobs::Table)
                        .col(column)
                        .to_owned(),
                )
                .await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("index_held_jobs_state_expiry")
                    .table(HeldJobs::Table)
                    .col(HeldJobs::State)
                    .col(HeldJobs::ExpiresAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(HeldJobs::Table).to_owned())
            .await
    }
}
