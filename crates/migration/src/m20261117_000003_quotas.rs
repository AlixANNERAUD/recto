//! Print quota rules and their adjustments.
//!
//! A rule's subject is everyone, a group, a user or a group's pool, so `subject_id` points to a
//! group or to a user depending on `subject_kind`, and no foreign key can guard it: the domain
//! deletes a group's rules with the group. A rule limited to some printers lists them in
//! `quota_rule_printers`, by the name CUPS gives them; without rows it covers every printer.
//!
//! Adjustments give one user (personal rules) or one group (pool rules) extra allowance for the
//! current window, or reset their usage from a point in time. Who made each one is kept.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Groups {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum QuotaRules {
    Table,
    Id,
    Name,
    Enabled,
    SubjectKind,
    SubjectId,
    Metric,
    Period,
    RollingDays,
    LimitValue,
    Enforcement,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum QuotaRulePrinters {
    Table,
    RuleId,
    Printer,
}

#[derive(DeriveIden)]
enum QuotaAdjustments {
    Table,
    Id,
    RuleId,
    UserId,
    GroupId,
    Kind,
    Amount,
    Reason,
    ByUserId,
    At,
    ExpiresAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(QuotaRules::Table)
                    .col(big_pk_auto(QuotaRules::Id))
                    .col(string(QuotaRules::Name))
                    .col(boolean(QuotaRules::Enabled))
                    // `everyone`, `group`, `user` or `pool`.
                    .col(string(QuotaRules::SubjectKind))
                    .col(big_integer_null(QuotaRules::SubjectId))
                    .col(string(QuotaRules::Metric))
                    .col(string(QuotaRules::Period))
                    // Only for the `rolling` period. Big integers throughout: 64 bits like
                    // SQLite, so entities read the same on PostgreSQL.
                    .col(big_integer_null(QuotaRules::RollingDays))
                    // In minor currency units for `cost`.
                    .col(big_integer(QuotaRules::LimitValue))
                    .col(string(QuotaRules::Enforcement))
                    .col(timestamp_with_time_zone(QuotaRules::CreatedAt))
                    .col(timestamp_with_time_zone(QuotaRules::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(QuotaRulePrinters::Table)
                    .col(big_integer(QuotaRulePrinters::RuleId))
                    .col(string(QuotaRulePrinters::Printer))
                    .primary_key(
                        Index::create()
                            .col(QuotaRulePrinters::RuleId)
                            .col(QuotaRulePrinters::Printer),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_quota_rule_printers_rule")
                            .from(QuotaRulePrinters::Table, QuotaRulePrinters::RuleId)
                            .to(QuotaRules::Table, QuotaRules::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(QuotaAdjustments::Table)
                    .col(big_pk_auto(QuotaAdjustments::Id))
                    .col(big_integer(QuotaAdjustments::RuleId))
                    // The user of a personal rule, or the group of a pool rule.
                    .col(big_integer_null(QuotaAdjustments::UserId))
                    .col(big_integer_null(QuotaAdjustments::GroupId))
                    // `grant` or `reset`.
                    .col(string(QuotaAdjustments::Kind))
                    // Grants only.
                    .col(big_integer_null(QuotaAdjustments::Amount))
                    .col(string(QuotaAdjustments::Reason))
                    .col(big_integer(QuotaAdjustments::ByUserId))
                    .col(timestamp_with_time_zone(QuotaAdjustments::At))
                    .col(timestamp_with_time_zone_null(QuotaAdjustments::ExpiresAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_quota_adjustments_rule")
                            .from(QuotaAdjustments::Table, QuotaAdjustments::RuleId)
                            .to(QuotaRules::Table, QuotaRules::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    // Like print jobs, adjustments are history: they keep their user.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_quota_adjustments_user")
                            .from(QuotaAdjustments::Table, QuotaAdjustments::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_quota_adjustments_group")
                            .from(QuotaAdjustments::Table, QuotaAdjustments::GroupId)
                            .to(Groups::Table, Groups::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_quota_adjustments_by")
                            .from(QuotaAdjustments::Table, QuotaAdjustments::ByUserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;
        // Usage reads the adjustments of one rule for one user or one group.
        for (name, column) in [
            (
                "index_quota_adjustments_rule_user",
                QuotaAdjustments::UserId,
            ),
            (
                "index_quota_adjustments_rule_group",
                QuotaAdjustments::GroupId,
            ),
        ] {
            manager
                .create_index(
                    Index::create()
                        .name(name)
                        .table(QuotaAdjustments::Table)
                        .col(QuotaAdjustments::RuleId)
                        .col(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(QuotaAdjustments::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(QuotaRulePrinters::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(QuotaRules::Table).to_owned())
            .await
    }
}
