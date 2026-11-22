//! Device passwords: the credentials computers and phones print and scan with. Each belongs to
//! one user and goes with them.
//!
//! Only the SHA-256 of the secret is kept, as lowercase hex. The secret is 160 random bits, so a
//! fast hash is enough, and it lets every request of a polling device authenticate with one
//! lookup of the unique index.

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
    UserId,
    Name,
    SecretHash,
    CreatedAt,
    LastUsedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DevicePasswords::Table)
                    .col(big_pk_auto(DevicePasswords::Id))
                    .col(big_integer(DevicePasswords::UserId))
                    .col(string(DevicePasswords::Name))
                    .col(string_uniq(DevicePasswords::SecretHash))
                    .col(timestamp_with_time_zone(DevicePasswords::CreatedAt))
                    .col(timestamp_with_time_zone_null(DevicePasswords::LastUsedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_device_passwords_user")
                            .from(DevicePasswords::Table, DevicePasswords::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // The Account page lists a user's passwords, and deleting a user cascades to them.
        manager
            .create_index(
                Index::create()
                    .name("index_device_passwords_user")
                    .table(DevicePasswords::Table)
                    .col(DevicePasswords::UserId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DevicePasswords::Table).to_owned())
            .await
    }
}
