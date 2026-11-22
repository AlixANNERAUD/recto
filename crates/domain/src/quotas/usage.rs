//! What a user, or a pool, has used of a rule in its current window.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use recto_entity::{group_memberships, print_jobs, quota_adjustments};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Alias, Expr, ExprTrait, Func, Query},
};

use super::{Holder, Metric, Rule, Subject, window};
use crate::error::Result;

/// Where someone stands against a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Standing {
    pub rule: Rule,
    /// Counted from the ledger in the current window, after the latest reset.
    pub used: i64,
    /// Extra allowance from grants that have not expired.
    pub granted: i64,
    /// `limit + granted - used`; negative when over.
    pub left: i64,
}

/// Where `user_id` stands against `rule` at `now`: their own usage for a personal rule, that of
/// every current member of the group for a pool.
///
/// Usage sums the jobs in the ledger that were not refunded, on the rule's printers, in the
/// window of the rule's period that contains `now`, from the latest reset on. A job whose counts
/// are unknown adds nothing, except to the jobs metric. Grants add to the limit until they
/// expire.
pub async fn standing(
    db: &impl ConnectionTrait,
    rule: &Rule,
    user_id: i64,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<Standing> {
    let holder = match rule.subject {
        Subject::Pool(group) => Holder::Group(group),
        _ => Holder::User(user_id),
    };
    let (window_start, end) = window(rule.period, now, tz);
    let reset = latest_reset(db, rule.id, holder, now).await?;

    let mut jobs = print_jobs::Entity::find()
        .select_only()
        .filter(print_jobs::Column::Refunded.eq(false));
    jobs = match holder {
        Holder::User(user) => jobs.filter(print_jobs::Column::UserId.eq(user)),
        // Membership as recorded now, at each member's last sign-in.
        Holder::Group(group) => jobs.filter(
            print_jobs::Column::UserId.in_subquery(
                Query::select()
                    .column(group_memberships::Column::UserId)
                    .from(group_memberships::Entity)
                    .and_where(group_memberships::Column::GroupId.eq(group))
                    .to_owned(),
            ),
        ),
    };
    if let Some(start) = window_start {
        jobs = jobs.filter(print_jobs::Column::SubmittedAt.gte(start.fixed_offset()));
    }
    if let Some(end) = end {
        jobs = jobs.filter(print_jobs::Column::SubmittedAt.lt(end.fixed_offset()));
    }
    // Only what was printed after the reset counts.
    if let Some(reset) = reset {
        jobs = jobs.filter(print_jobs::Column::SubmittedAt.gt(reset.fixed_offset()));
    }
    if !rule.printers.is_empty() {
        // CUPS compares printer names without case. Both sides go through the database's own
        // LOWER, which folds only A to Z on SQLite but every letter on PostgreSQL.
        let printers = rule
            .printers
            .iter()
            .map(|printer| Expr::expr(Func::lower(Expr::val(printer.as_str()))));
        jobs = jobs.filter(
            Expr::expr(Func::lower(Expr::col((
                print_jobs::Entity,
                print_jobs::Column::Printer,
            ))))
            .is_in(printers),
        );
    }
    let total: Expr = match rule.metric {
        Metric::Jobs => Func::count(Expr::col((print_jobs::Entity, print_jobs::Column::Id))).into(),
        metric => sum(Expr::col((print_jobs::Entity, column(metric)))),
    };
    let used = jobs
        .expr_as(total, "total")
        .into_tuple::<Option<i64>>()
        .one(db)
        .await?
        .flatten()
        .unwrap_or(0);

    let granted = quota_adjustments::Entity::find()
        .select_only()
        .expr_as(
            sum(Expr::col((
                quota_adjustments::Entity,
                quota_adjustments::Column::Amount,
            ))),
            "total",
        )
        .filter(quota_adjustments::Column::RuleId.eq(rule.id))
        .filter(holder_filter(holder))
        .filter(quota_adjustments::Column::Kind.eq("grant"))
        .filter(quota_adjustments::Column::At.lte(now.fixed_offset()))
        .filter(
            quota_adjustments::Column::ExpiresAt
                .is_null()
                .or(quota_adjustments::Column::ExpiresAt.gt(now.fixed_offset())),
        )
        .into_tuple::<Option<i64>>()
        .one(db)
        .await?
        .flatten()
        .unwrap_or(0);

    Ok(Standing {
        rule: rule.clone(),
        used,
        granted,
        left: rule.limit.saturating_add(granted).saturating_sub(used),
    })
}

/// `SUM` as a 64-bit integer: PostgreSQL sums `bigint` into `numeric`, which does not read back
/// as one.
fn sum(expression: Expr) -> Expr {
    Func::cast_as(Func::sum(expression), Alias::new("BIGINT")).into()
}

fn column(metric: Metric) -> print_jobs::Column {
    match metric {
        Metric::Pages => print_jobs::Column::Pages,
        Metric::Sheets => print_jobs::Column::Sheets,
        Metric::ColorPages => print_jobs::Column::ColorPages,
        Metric::MonoPages => print_jobs::Column::MonoPages,
        Metric::Cost => print_jobs::Column::Cost,
        Metric::Jobs => print_jobs::Column::Id,
    }
}

fn holder_filter(holder: Holder) -> sea_orm::Condition {
    match holder {
        Holder::User(user) => sea_orm::Condition::all()
            .add(quota_adjustments::Column::UserId.eq(user))
            .add(quota_adjustments::Column::GroupId.is_null()),
        Holder::Group(group) => sea_orm::Condition::all()
            .add(quota_adjustments::Column::GroupId.eq(group))
            .add(quota_adjustments::Column::UserId.is_null()),
    }
}

/// When `holder`'s usage of the rule was last reset, if ever, up to `now`.
async fn latest_reset(
    db: &impl ConnectionTrait,
    rule_id: i64,
    holder: Holder,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    let latest = quota_adjustments::Entity::find()
        .filter(quota_adjustments::Column::RuleId.eq(rule_id))
        .filter(holder_filter(holder))
        .filter(quota_adjustments::Column::Kind.eq("reset"))
        .filter(quota_adjustments::Column::At.lte(now.fixed_offset()))
        .order_by_desc(quota_adjustments::Column::At)
        .one(db)
        .await?;
    Ok(latest.map(|reset| reset.at.with_timezone(&Utc)))
}
