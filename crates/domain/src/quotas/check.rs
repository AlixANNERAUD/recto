//! Whether a job may print, decided inside the write transaction that enters it in the ledger.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use recto_entity::groups;
use sea_orm::{DatabaseTransaction, EntityTrait};

#[cfg(doc)]
use super::applicable;
use super::{Enforcement, Rule, amount, standing, usage::Standing};
use crate::{
    database::{lock, lock_user},
    error::Result,
    ledger::Counts,
};

/// What the quotas say of a job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It may print; the soft rules it goes over, if any, call for a warning.
    Allowed { warnings: Vec<Standing> },
    /// A hard rule has too little left.
    Refused { standing: Standing },
}

/// Checks a job against the rules that apply to it ([`applicable`]).
///
/// A rule the job counts nothing for (no color pages under a color rule, a free printer under a
/// cost rule) never stops it, even when the rule is already over. Otherwise the job goes over a
/// rule when it counts for more than is left: a hard rule refuses it (the first such rule by
/// id), a soft rule only warns. A job that uses exactly what is left passes. Counts recto could
/// not read count for nothing here: the caller refuses such a job beforehand when a rule needs
/// them.
///
/// Concurrent jobs must not both pass: call it in a [`begin_write`](crate::database::begin_write)
/// transaction, after [`lock_subjects`], and enter the job in the ledger before committing.
///
/// `applicable` are the rules [`applicable`] chose from the rules read in this transaction, the
/// same ones whose pools [`lock_subjects`] locked: read again here, on PostgreSQL they could
/// include a pool rule enabled since, whose group nobody locked.
pub async fn check(
    tx: &DatabaseTransaction,
    user_id: i64,
    applicable: &[Rule],
    counts: &Counts,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<Verdict> {
    let mut warnings = Vec::new();
    for rule in applicable {
        let job = amount(rule.metric, counts).unwrap_or(0);
        if job <= 0 {
            continue;
        }
        let standing = standing(tx, rule, user_id, now, tz).await?;
        if standing.left >= job {
            continue;
        }
        match rule.enforcement {
            Enforcement::Hard => return Ok(Verdict::Refused { standing }),
            Enforcement::Soft => warnings.push(standing),
        }
    }
    Ok(Verdict::Allowed { warnings })
}

/// Makes concurrent checks of the same user, or of the same pools, wait for each other.
///
/// On PostgreSQL, whose default isolation lets two transactions read the same usage, it locks
/// the user's row, then each pool's group row by ascending id, with `SELECT … FOR UPDATE`: two
/// jobs that share a budget always share a row to wait on, and the fixed order keeps them from
/// deadlocking. The locks hold until the transaction ends. On SQLite it does nothing: the write
/// transaction already runs alone.
pub async fn lock_subjects(
    tx: &DatabaseTransaction,
    user_id: i64,
    pool_groups: &[i64],
) -> Result<()> {
    lock_user(tx, user_id).await?;
    let mut pool_groups = pool_groups.to_vec();
    pool_groups.sort_unstable();
    pool_groups.dedup();
    for group in pool_groups {
        lock(tx, groups::Entity::find_by_id(group)).await?;
    }
    Ok(())
}
