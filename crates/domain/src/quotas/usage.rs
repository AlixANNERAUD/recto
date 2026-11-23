//! What a user, or a pool, has used of a rule in its current window.

use std::{collections::HashMap, hash::Hash};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use recto_entity::{group_memberships, print_jobs, quota_adjustments};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect, Select,
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
    let mut standings = standings_of(db, std::slice::from_ref(rule), user_id, now, tz).await?;
    Ok(standings.remove(0))
}

/// [`standing`] of `user_id` against each of `rules`, in their order, for a page showing where
/// they stand: the adjustments of every rule are read at once, then each rule's usage with one
/// query.
pub async fn standings_of(
    db: &impl ConnectionTrait,
    rules: &[Rule],
    user_id: i64,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<Vec<Standing>> {
    let holder = |rule: &Rule| match rule.subject {
        Subject::Pool(group) => Holder::Group(group),
        _ => Holder::User(user_id),
    };
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let of_holders = rules.iter().fold(Condition::any(), |any, rule| {
        any.add(
            Condition::all()
                .add(quota_adjustments::Column::RuleId.eq(rule.id))
                .add(holder_filter(holder(rule))),
        )
    });
    let adjustments = quota_adjustments::Entity::find()
        .filter(of_holders)
        .filter(quota_adjustments::Column::At.lte(now.fixed_offset()))
        .all(db)
        .await?;
    let (resets, granted) = tally(adjustments, |adjustment| Some(adjustment.rule_id), now);
    let mut standings = Vec::with_capacity(rules.len());
    for rule in rules {
        let reset = resets.get(&rule.id).copied();
        let used = self::used(db, rule, holder(rule), reset, now, tz).await?;
        let granted = granted.get(&rule.id).copied().unwrap_or(0);
        standings.push(Standing::new(rule, used, granted));
    }
    Ok(standings)
}

/// The latest reset and the total of the grants still running at `now` among `adjustments`, by
/// `key`; an adjustment without a key counts for nothing.
fn tally<K: Copy + Eq + Hash>(
    adjustments: Vec<quota_adjustments::Model>,
    key: impl Fn(&quota_adjustments::Model) -> Option<K>,
    now: DateTime<Utc>,
) -> (HashMap<K, DateTime<Utc>>, HashMap<K, i64>) {
    let mut resets: HashMap<K, DateTime<Utc>> = HashMap::new();
    let mut granted: HashMap<K, i64> = HashMap::new();
    for adjustment in adjustments {
        let Some(key) = key(&adjustment) else {
            continue;
        };
        let at = adjustment.at.with_timezone(&Utc);
        match adjustment.kind.as_str() {
            "reset" => {
                let latest = resets.entry(key).or_insert(at);
                *latest = Ord::max(*latest, at);
            }
            "grant" if adjustment.expires_at.is_none_or(|expiry| expiry > now) => {
                let total = granted.entry(key).or_default();
                *total = total.saturating_add(adjustment.amount.unwrap_or(0));
            }
            _ => {}
        }
    }
    (resets, granted)
}

/// [`standing`] of each of `users` against `rule`, for a page listing every user. A personal
/// rule reads the adjustments of all of them at once and sums the usage of those never reset in
/// one grouped query; each user with a reset gets a query of their own, since the window they
/// count from differs.
pub async fn standings(
    db: &impl ConnectionTrait,
    rule: &Rule,
    users: &[i64],
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<HashMap<i64, Standing>> {
    let Some(first) = users.first() else {
        return Ok(HashMap::new());
    };
    if let Subject::Pool(_) = rule.subject {
        // One budget, the same for every member.
        let pool = standing(db, rule, *first, now, tz).await?;
        return Ok(users.iter().map(|user| (*user, pool.clone())).collect());
    }
    let adjustments = quota_adjustments::Entity::find()
        .filter(quota_adjustments::Column::RuleId.eq(rule.id))
        .filter(quota_adjustments::Column::GroupId.is_null())
        .filter(quota_adjustments::Column::UserId.is_in(users.iter().copied()))
        .filter(quota_adjustments::Column::At.lte(now.fixed_offset()))
        .all(db)
        .await?;
    let (resets, granted) = tally(adjustments, |adjustment| adjustment.user_id, now);

    let never_reset: Vec<i64> = users
        .iter()
        .copied()
        .filter(|user| !resets.contains_key(user))
        .collect();
    let mut used: HashMap<i64, i64> = if never_reset.is_empty() {
        HashMap::new()
    } else {
        counted(rule, now, tz)
            .column(print_jobs::Column::UserId)
            .expr_as(total(rule.metric), "total")
            .filter(print_jobs::Column::UserId.is_in(never_reset))
            .group_by(print_jobs::Column::UserId)
            .into_tuple::<(i64, Option<i64>)>()
            .all(db)
            .await?
            .into_iter()
            .map(|(user, total)| (user, total.unwrap_or(0)))
            .collect()
    };
    for (user, reset) in &resets {
        let total = self::used(db, rule, Holder::User(*user), Some(*reset), now, tz).await?;
        used.insert(*user, total);
    }
    Ok(users
        .iter()
        .map(|user| {
            let used = used.get(user).copied().unwrap_or(0);
            let granted = granted.get(user).copied().unwrap_or(0);
            (*user, Standing::new(rule, used, granted))
        })
        .collect())
}

impl Standing {
    fn new(rule: &Rule, used: i64, granted: i64) -> Self {
        Standing {
            rule: rule.clone(),
            used,
            granted,
            left: rule.limit.saturating_add(granted).saturating_sub(used),
        }
    }
}

/// What `holder` used of `rule` in its window at `now`, counting only jobs after `reset`.
async fn used(
    db: &impl ConnectionTrait,
    rule: &Rule,
    holder: Holder,
    reset: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<i64> {
    let mut jobs = counted(rule, now, tz);
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
    // Only what was printed after the reset counts.
    if let Some(reset) = reset {
        jobs = jobs.filter(print_jobs::Column::SubmittedAt.gt(reset.fixed_offset()));
    }
    Ok(jobs
        .expr_as(total(rule.metric), "total")
        .into_tuple::<Option<i64>>()
        .one(db)
        .await?
        .flatten()
        .unwrap_or(0))
}

/// The jobs `rule` counts at `now`, whoever printed them, selecting nothing yet: not refunded,
/// on the rule's printers, in the window of its period.
fn counted(rule: &Rule, now: DateTime<Utc>, tz: Tz) -> Select<print_jobs::Entity> {
    let (start, end) = window(rule.period, now, tz);
    let mut jobs = print_jobs::Entity::find()
        .select_only()
        .filter(print_jobs::Column::Refunded.eq(false));
    if let Some(start) = start {
        jobs = jobs.filter(print_jobs::Column::SubmittedAt.gte(start.fixed_offset()));
    }
    if let Some(end) = end {
        jobs = jobs.filter(print_jobs::Column::SubmittedAt.lt(end.fixed_offset()));
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
    jobs
}

/// What a job adds to `metric`, summed.
fn total(metric: Metric) -> Expr {
    match metric {
        Metric::Jobs => Func::count(Expr::col((print_jobs::Entity, print_jobs::Column::Id))).into(),
        metric => sum(Expr::col((print_jobs::Entity, column(metric)))),
    }
}

/// `SUM` as a 64-bit integer: PostgreSQL sums `bigint` into `numeric`, which does not read back
/// as one.
pub(crate) fn sum(expression: Expr) -> Expr {
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
