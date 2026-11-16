//! Print quotas: rules over the ledger that limit what people print, and the adjustments
//! administrators make to them.
//!
//! A rule limits one metric of the ledger (pages, sheets, color or black-and-white pages, jobs
//! or cost) over a period, for everyone, a group's members each, one user, or a group's members
//! together (a pool), on every printer or some. [`applicable()`] picks the rules a job must pass,
//! [`standing()`] sums what a user or a pool used in a rule's current [`window()`], and [`check()`]
//! decides, inside the write transaction that enters the job in the ledger, whether it may
//! print.
//!
//! A rule's subject is a group or a user by id, which the database cannot guard with a foreign
//! key: [`groups::delete`](crate::groups::delete) deletes the group's rules with it. recto never
//! deletes users.

mod check;
mod family;
mod usage;
mod window;

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use recto_entity::{groups, quota_adjustments, quota_rule_printers, quota_rules, users};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, QueryOrder,
};

pub use check::{Verdict, check, lock_subjects};
pub use family::{amount, applicable};
pub use usage::{Standing, standing};
pub use window::window;

use crate::{Error, Problem, database::begin_write, error::Result};

/// Whom a rule limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Subject {
    /// Each person.
    Everyone,
    /// Each member of the group, by id.
    Group(i64),
    /// One user, by id.
    User(i64),
    /// The members of the group together, by group id: one shared budget.
    Pool(i64),
}

/// What a rule counts, from the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Metric {
    Pages,
    Sheets,
    ColorPages,
    MonoPages,
    Jobs,
    /// In minor currency units.
    Cost,
}

/// The window a rule counts in. Calendar periods follow `RECTO_TIMEZONE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Period {
    Day,
    /// From Monday.
    Week,
    Month,
    Quarter,
    Year,
    /// The last so many days, of 24 hours, from 1 to 366.
    Rolling(u16),
    /// Never resets.
    Total,
}

/// What happens to a job over the limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Enforcement {
    /// It is refused.
    Hard,
    /// It prints, with a warning.
    Soft,
}

/// A quota rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub subject: Subject,
    /// CUPS printer names; empty for every printer.
    pub printers: Vec<String>,
    pub metric: Metric,
    pub period: Period,
    /// In the metric's unit; minor currency units for cost.
    pub limit: i64,
    pub enforcement: Enforcement,
}

/// A rule to create, or what to change a rule into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRule {
    pub name: String,
    pub enabled: bool,
    pub subject: Subject,
    pub printers: Vec<String>,
    pub metric: Metric,
    pub period: Period,
    pub limit: i64,
    pub enforcement: Enforcement,
}

/// Who an adjustment is for: the user, for a personal rule, or the group of a pool rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Holder {
    User(i64),
    Group(i64),
}

impl Subject {
    fn kind(self) -> &'static str {
        match self {
            Subject::Everyone => "everyone",
            Subject::Group(_) => "group",
            Subject::User(_) => "user",
            Subject::Pool(_) => "pool",
        }
    }

    fn id(self) -> Option<i64> {
        match self {
            Subject::Everyone => None,
            Subject::Group(id) | Subject::User(id) | Subject::Pool(id) => Some(id),
        }
    }

    fn read(kind: &str, id: Option<i64>) -> Option<Self> {
        Some(match (kind, id) {
            ("everyone", _) => Subject::Everyone,
            ("group", Some(id)) => Subject::Group(id),
            ("user", Some(id)) => Subject::User(id),
            ("pool", Some(id)) => Subject::Pool(id),
            _ => return None,
        })
    }
}

impl Metric {
    const ALL: [Metric; 6] = [
        Metric::Pages,
        Metric::Sheets,
        Metric::ColorPages,
        Metric::MonoPages,
        Metric::Jobs,
        Metric::Cost,
    ];

    /// The keyword the database stores, as forms post it.
    pub fn keyword(self) -> &'static str {
        match self {
            Metric::Pages => "pages",
            Metric::Sheets => "sheets",
            Metric::ColorPages => "color_pages",
            Metric::MonoPages => "mono_pages",
            Metric::Jobs => "jobs",
            Metric::Cost => "cost",
        }
    }

    pub fn from_keyword(keyword: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|metric| metric.keyword() == keyword)
    }
}

impl Period {
    /// The keyword the database stores, as forms post it; rolling periods keep their days apart.
    pub fn keyword(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
            Period::Quarter => "quarter",
            Period::Year => "year",
            Period::Rolling(_) => "rolling",
            Period::Total => "total",
        }
    }

    /// The period of a keyword, with the days of a rolling one.
    pub fn from_keyword(keyword: &str, rolling_days: Option<u16>) -> Option<Self> {
        Some(match keyword {
            "day" => Period::Day,
            "week" => Period::Week,
            "month" => Period::Month,
            "quarter" => Period::Quarter,
            "year" => Period::Year,
            "rolling" => Period::Rolling(rolling_days?),
            "total" => Period::Total,
            _ => return None,
        })
    }

    fn rolling_days(self) -> Option<i64> {
        match self {
            Period::Rolling(days) => Some(i64::from(days)),
            _ => None,
        }
    }
}

impl Enforcement {
    pub fn keyword(self) -> &'static str {
        match self {
            Enforcement::Hard => "hard",
            Enforcement::Soft => "soft",
        }
    }

    pub fn from_keyword(keyword: &str) -> Option<Self> {
        match keyword {
            "hard" => Some(Enforcement::Hard),
            "soft" => Some(Enforcement::Soft),
            _ => None,
        }
    }
}

/// The longest rolling period, in days: a leap year.
pub const MAX_ROLLING_DAYS: u16 = 366;

/// A stored value recto does not know: the row was written by another version or by hand.
fn unreadable(rule: i64, what: &str) -> Error {
    Error::Database(DbErr::Custom(format!(
        "quota rule {rule} has an unknown {what}"
    )))
}

fn rule_of(row: quota_rules::Model, printers: Vec<String>) -> Result<Rule> {
    let rolling_days = row.rolling_days.and_then(|days| u16::try_from(days).ok());
    Ok(Rule {
        id: row.id,
        subject: Subject::read(&row.subject_kind, row.subject_id)
            .ok_or_else(|| unreadable(row.id, "subject"))?,
        metric: Metric::from_keyword(&row.metric).ok_or_else(|| unreadable(row.id, "metric"))?,
        period: Period::from_keyword(&row.period, rolling_days)
            .ok_or_else(|| unreadable(row.id, "period"))?,
        enforcement: Enforcement::from_keyword(&row.enforcement)
            .ok_or_else(|| unreadable(row.id, "enforcement"))?,
        name: row.name,
        enabled: row.enabled,
        printers,
        limit: row.limit_value,
    })
}

/// Every rule, by id, with its printers by name.
pub async fn rules(db: &impl ConnectionTrait) -> Result<Vec<Rule>> {
    let rows = quota_rules::Entity::find()
        .order_by_asc(quota_rules::Column::Id)
        .all(db)
        .await?;
    let mut printers: HashMap<i64, Vec<String>> = HashMap::new();
    if !rows.is_empty() {
        let links = quota_rule_printers::Entity::find()
            .order_by_asc(quota_rule_printers::Column::Printer)
            .all(db)
            .await?;
        for link in links {
            printers.entry(link.rule_id).or_default().push(link.printer);
        }
    }
    rows.into_iter()
        .map(|row| {
            let printers = printers.remove(&row.id).unwrap_or_default();
            rule_of(row, printers)
        })
        .collect()
}

/// One rule.
pub async fn rule(db: &impl ConnectionTrait, id: i64) -> Result<Rule> {
    let row = quota_rules::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or(Error::NotFound)?;
    let printers = quota_rule_printers::Entity::find()
        .filter(quota_rule_printers::Column::RuleId.eq(id))
        .order_by_asc(quota_rule_printers::Column::Printer)
        .all(db)
        .await?
        .into_iter()
        .map(|link| link.printer)
        .collect();
    rule_of(row, printers)
}

/// Checks a rule and returns it tidied: the name and printer names trimmed, each printer once
/// (without case, as CUPS compares them).
async fn validated(db: &impl ConnectionTrait, rule: NewRule) -> Result<NewRule> {
    let invalid = |problem| Err(Error::Invalid(problem));
    let name = rule.name.trim().to_owned();
    if name.is_empty() {
        return invalid(Problem::RuleNameEmpty);
    }
    if rule.limit < 0 {
        return invalid(Problem::RuleLimitNegative);
    }
    if let Period::Rolling(days) = rule.period
        && !(1..=MAX_ROLLING_DAYS).contains(&days)
    {
        return invalid(Problem::RollingDaysInvalid {
            maximum: MAX_ROLLING_DAYS,
        });
    }
    let mut printers: Vec<String> = Vec::new();
    for printer in &rule.printers {
        let printer = printer.trim();
        if printer.is_empty() {
            return invalid(Problem::PrinterNameEmpty);
        }
        if !printers
            .iter()
            .any(|known| known.eq_ignore_ascii_case(printer))
        {
            printers.push(printer.to_owned());
        }
    }
    let exists = match rule.subject {
        Subject::Everyone => true,
        Subject::Group(id) | Subject::Pool(id) => {
            groups::Entity::find_by_id(id).one(db).await?.is_some()
        }
        Subject::User(id) => users::Entity::find_by_id(id).one(db).await?.is_some(),
    };
    if !exists {
        return invalid(Problem::SubjectNotFound);
    }
    Ok(NewRule {
        name,
        printers,
        ..rule
    })
}

async fn insert_printers(
    db: &impl ConnectionTrait,
    rule_id: i64,
    printers: &[String],
) -> Result<()> {
    for printer in printers {
        quota_rule_printers::ActiveModel {
            rule_id: Set(rule_id),
            printer: Set(printer.clone()),
        }
        .insert(db)
        .await?;
    }
    Ok(())
}

/// Creates a rule; returns its id.
pub async fn create(db: &DatabaseConnection, rule: NewRule) -> Result<i64> {
    let transaction = begin_write(db).await?;
    let rule = validated(&transaction, rule).await?;
    let now = Utc::now().fixed_offset();
    let row = quota_rules::ActiveModel {
        name: Set(rule.name),
        enabled: Set(rule.enabled),
        subject_kind: Set(rule.subject.kind().to_owned()),
        subject_id: Set(rule.subject.id()),
        metric: Set(rule.metric.keyword().to_owned()),
        period: Set(rule.period.keyword().to_owned()),
        rolling_days: Set(rule.period.rolling_days()),
        limit_value: Set(rule.limit),
        enforcement: Set(rule.enforcement.keyword().to_owned()),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(&transaction)
    .await?;
    insert_printers(&transaction, row.id, &rule.printers).await?;
    transaction.commit().await?;
    Ok(row.id)
}

/// Changes every field of a rule. Its adjustments stay.
pub async fn update(db: &DatabaseConnection, id: i64, rule: NewRule) -> Result<()> {
    let transaction = begin_write(db).await?;
    let row = quota_rules::Entity::find_by_id(id)
        .one(&transaction)
        .await?
        .ok_or(Error::NotFound)?;
    let rule = validated(&transaction, rule).await?;
    let mut row: quota_rules::ActiveModel = row.into();
    row.name = Set(rule.name);
    row.enabled = Set(rule.enabled);
    row.subject_kind = Set(rule.subject.kind().to_owned());
    row.subject_id = Set(rule.subject.id());
    row.metric = Set(rule.metric.keyword().to_owned());
    row.period = Set(rule.period.keyword().to_owned());
    row.rolling_days = Set(rule.period.rolling_days());
    row.limit_value = Set(rule.limit);
    row.enforcement = Set(rule.enforcement.keyword().to_owned());
    row.updated_at = Set(Utc::now().fixed_offset());
    row.update(&transaction).await?;
    quota_rule_printers::Entity::delete_many()
        .filter(quota_rule_printers::Column::RuleId.eq(id))
        .exec(&transaction)
        .await?;
    insert_printers(&transaction, id, &rule.printers).await?;
    transaction.commit().await?;
    Ok(())
}

/// Deletes a rule, with its printers and adjustments.
pub async fn delete(db: &impl ConnectionTrait, id: i64) -> Result<()> {
    let deleted = quota_rules::Entity::delete_by_id(id).exec(db).await?;
    if deleted.rows_affected == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

/// Deletes the rules of a group, its pool's included; the group is about to go.
pub(crate) async fn delete_rules_of_group(db: &impl ConnectionTrait, group_id: i64) -> Result<()> {
    quota_rules::Entity::delete_many()
        .filter(quota_rules::Column::SubjectKind.is_in(["group", "pool"]))
        .filter(quota_rules::Column::SubjectId.eq(group_id))
        .exec(db)
        .await?;
    Ok(())
}

/// The rule, after checking that `holder` is whom its adjustments are for.
async fn adjusted(db: &impl ConnectionTrait, rule_id: i64, holder: Holder) -> Result<Rule> {
    let rule = rule(db, rule_id).await?;
    let fits = match (rule.subject, holder) {
        (Subject::Pool(group), Holder::Group(holder)) => group == holder,
        (Subject::User(user), Holder::User(holder)) => user == holder,
        (Subject::Pool(_), Holder::User(_)) | (_, Holder::Group(_)) => false,
        (Subject::Everyone | Subject::Group(_), Holder::User(_)) => true,
    };
    if !fits {
        return Err(Error::Invalid(Problem::HolderMismatch));
    }
    let exists = match holder {
        Holder::User(id) => users::Entity::find_by_id(id).one(db).await?.is_some(),
        Holder::Group(id) => groups::Entity::find_by_id(id).one(db).await?.is_some(),
    };
    if !exists {
        return Err(Error::Invalid(Problem::SubjectNotFound));
    }
    Ok(rule)
}

/// When a grant made at `at` stops counting: at the end of the rule's window, `days` after it for
/// a rolling rule, never for a total.
pub fn grant_expiry(period: Period, at: DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>> {
    match period {
        Period::Total => None,
        Period::Rolling(days) => Some(at + Duration::days(i64::from(days))),
        _ => window(period, at, tz).1,
    }
}

fn holder_columns(holder: Holder) -> (Option<i64>, Option<i64>) {
    match holder {
        Holder::User(id) => (Some(id), None),
        Holder::Group(id) => (None, Some(id)),
    }
}

/// Gives `holder` `amount` more of the rule for its current window (see [`grant_expiry`]).
#[allow(clippy::too_many_arguments)]
pub async fn grant(
    db: &impl ConnectionTrait,
    rule_id: i64,
    holder: Holder,
    amount: i64,
    reason: &str,
    by: i64,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<()> {
    if amount <= 0 {
        return Err(Error::Invalid(Problem::GrantNotPositive));
    }
    let rule = adjusted(db, rule_id, holder).await?;
    let (user_id, group_id) = holder_columns(holder);
    quota_adjustments::ActiveModel {
        rule_id: Set(rule_id),
        user_id: Set(user_id),
        group_id: Set(group_id),
        kind: Set("grant".to_owned()),
        amount: Set(Some(amount)),
        reason: Set(reason.trim().to_owned()),
        by_user_id: Set(by),
        at: Set(now.fixed_offset()),
        expires_at: Set(grant_expiry(rule.period, now, tz).map(|time| time.fixed_offset())),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(())
}

/// Starts `holder`'s usage of the rule over from `now`: only what they print afterwards counts.
pub async fn reset(
    db: &impl ConnectionTrait,
    rule_id: i64,
    holder: Holder,
    reason: &str,
    by: i64,
    now: DateTime<Utc>,
) -> Result<()> {
    adjusted(db, rule_id, holder).await?;
    let (user_id, group_id) = holder_columns(holder);
    quota_adjustments::ActiveModel {
        rule_id: Set(rule_id),
        user_id: Set(user_id),
        group_id: Set(group_id),
        kind: Set("reset".to_owned()),
        amount: Set(None),
        reason: Set(reason.trim().to_owned()),
        by_user_id: Set(by),
        at: Set(now.fixed_offset()),
        expires_at: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(())
}

/// The ids of the groups pools of `rules` belong to, ascending, once each: what
/// [`lock_subjects`] locks.
pub fn pool_groups(rules: &[Rule]) -> Vec<i64> {
    rules
        .iter()
        .filter_map(|rule| match rule.subject {
            Subject::Pool(group) => Some(group),
            _ => None,
        })
        .collect::<BTreeSet<i64>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests;
