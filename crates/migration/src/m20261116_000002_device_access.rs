//! Which groups may use a printer or a scanner. A device without a row is open to everyone.
//!
//! Devices are named by their service (a CUPS printer name, a SANE device id), not by a row of
//! ours, so the rule keeps the name. Deleting a group removes it from every rule, which leaves
//! the device open only to the remaining groups (and administrators).

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Groups {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum DeviceAccess {
    Table,
    Id,
    Kind,
    Device,
    CreatedAt,
}

#[derive(DeriveIden)]
enum DeviceAccessGroups {
    Table,
    AccessId,
    GroupId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DeviceAccess::Table)
                    .col(big_pk_auto(DeviceAccess::Id))
                    // `printer` or `scanner`: a printer and a scanner may share a name.
                    .col(string(DeviceAccess::Kind))
                    .col(string(DeviceAccess::Device))
                    .col(timestamp_with_time_zone(DeviceAccess::CreatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("index_device_access_kind_device")
                    .table(DeviceAccess::Table)
                    .col(DeviceAccess::Kind)
                    .col(DeviceAccess::Device)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(DeviceAccessGroups::Table)
                    .col(big_integer(DeviceAccessGroups::AccessId))
                    .col(big_integer(DeviceAccessGroups::GroupId))
                    .primary_key(
                        Index::create()
                            .col(DeviceAccessGroups::AccessId)
                            .col(DeviceAccessGroups::GroupId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_device_access_groups_access")
                            .from(DeviceAccessGroups::Table, DeviceAccessGroups::AccessId)
                            .to(DeviceAccess::Table, DeviceAccess::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_device_access_groups_group")
                            .from(DeviceAccessGroups::Table, DeviceAccessGroups::GroupId)
                            .to(Groups::Table, Groups::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // The primary key serves the cascade from a rule; this one serves it from a group.
        manager
            .create_index(
                Index::create()
                    .name("index_device_access_groups_group")
                    .table(DeviceAccessGroups::Table)
                    .col(DeviceAccessGroups::GroupId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DeviceAccessGroups::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(DeviceAccess::Table).to_owned())
            .await
    }
}
