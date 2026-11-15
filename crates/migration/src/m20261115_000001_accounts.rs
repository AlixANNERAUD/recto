//! The initial schema: users, the identities they sign in with, groups and sessions.
//! Timestamps are stored with their time zone so the same schema works on SQLite and PostgreSQL.
//!
//! Deleting a user or a group cascades to its identities and memberships.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
    Username,
    DisplayName,
    Email,
    PasswordHash,
    IsAdministrator,
    AdministratorFromClaim,
    IsDisabled,
    SessionsValidAfter,
    CreatedAt,
    LastSignInAt,
}

#[derive(DeriveIden)]
enum UserIdentities {
    Table,
    Id,
    UserId,
    Issuer,
    Subject,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Groups {
    Table,
    Id,
    Name,
    CreatedAt,
}

#[derive(DeriveIden)]
enum GroupMemberships {
    Table,
    Id,
    GroupId,
    UserId,
}

#[derive(DeriveIden)]
enum Sessions {
    Table,
    Id,
    Data,
    ExpiryDate,
}

fn foreign_key(
    name: &str,
    from: (impl IntoIden, impl IntoIden),
    to: (impl IntoIden, impl IntoIden),
    on_delete: ForeignKeyAction,
) -> ForeignKeyCreateStatement {
    ForeignKey::create()
        .name(name)
        .from(from.0, from.1)
        .to(to.0, to.1)
        .on_delete(on_delete)
        .to_owned()
}

fn index(name: &str, table: impl IntoIden, columns: &[&'static str]) -> IndexCreateStatement {
    let mut statement = Index::create().name(name).table(table).to_owned();
    for column in columns {
        statement.col(Alias::new(*column));
    }
    statement
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .col(big_pk_auto(Users::Id))
                    .col(string_uniq(Users::Username))
                    .col(string(Users::DisplayName))
                    .col(string_null(Users::Email).unique_key())
                    .col(string_null(Users::PasswordHash))
                    .col(boolean(Users::IsAdministrator).default(false))
                    .col(boolean(Users::AdministratorFromClaim).default(false))
                    .col(boolean(Users::IsDisabled).default(false))
                    .col(timestamp_with_time_zone_null(Users::SessionsValidAfter))
                    .col(timestamp_with_time_zone(Users::CreatedAt))
                    .col(timestamp_with_time_zone_null(Users::LastSignInAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserIdentities::Table)
                    .col(big_pk_auto(UserIdentities::Id))
                    .col(big_integer(UserIdentities::UserId))
                    .col(string(UserIdentities::Issuer))
                    .col(string(UserIdentities::Subject))
                    .col(timestamp_with_time_zone(UserIdentities::CreatedAt))
                    .foreign_key(&mut foreign_key(
                        "fk_user_identities_user",
                        (UserIdentities::Table, UserIdentities::UserId),
                        (Users::Table, Users::Id),
                        ForeignKeyAction::Cascade,
                    ))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                index(
                    "index_user_identities_issuer_subject",
                    UserIdentities::Table,
                    &["issuer", "subject"],
                )
                .unique()
                .to_owned(),
            )
            .await?;
        // The unique index above leads with the issuer; finding a user's identities, and the
        // cascade when a user is deleted, need their own.
        manager
            .create_index(index(
                "index_user_identities_user",
                UserIdentities::Table,
                &["user_id"],
            ))
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Groups::Table)
                    .col(big_pk_auto(Groups::Id))
                    .col(string_uniq(Groups::Name))
                    .col(timestamp_with_time_zone(Groups::CreatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(GroupMemberships::Table)
                    .col(big_pk_auto(GroupMemberships::Id))
                    .col(big_integer(GroupMemberships::GroupId))
                    .col(big_integer(GroupMemberships::UserId))
                    .foreign_key(&mut foreign_key(
                        "fk_group_memberships_group",
                        (GroupMemberships::Table, GroupMemberships::GroupId),
                        (Groups::Table, Groups::Id),
                        ForeignKeyAction::Cascade,
                    ))
                    .foreign_key(&mut foreign_key(
                        "fk_group_memberships_user",
                        (GroupMemberships::Table, GroupMemberships::UserId),
                        (Users::Table, Users::Id),
                        ForeignKeyAction::Cascade,
                    ))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                index(
                    "index_group_memberships_group_user",
                    GroupMemberships::Table,
                    &["group_id", "user_id"],
                )
                .unique()
                .to_owned(),
            )
            .await?;
        // The unique index above leads with the group; listing a user's groups needs its own.
        manager
            .create_index(index(
                "index_group_memberships_user",
                GroupMemberships::Table,
                &["user_id"],
            ))
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Sessions::Table)
                    .col(string(Sessions::Id).primary_key())
                    .col(text(Sessions::Data))
                    .col(timestamp_with_time_zone(Sessions::ExpiryDate))
                    .to_owned(),
            )
            .await?;
        // Expired sessions are deleted by expiry date.
        manager
            .create_index(index(
                "index_sessions_expiry_date",
                Sessions::Table,
                &["expiry_date"],
            ))
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            Sessions::Table.into_iden(),
            GroupMemberships::Table.into_iden(),
            Groups::Table.into_iden(),
            UserIdentities::Table.into_iden(),
            Users::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }
}
