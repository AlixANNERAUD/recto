//! Held print jobs: documents computers and phones sent to recto, waiting for their owner to
//! release them to a printer, to delete them, or for them to expire. Nothing is counted while a
//! job is held: the ledger only knows printed jobs.
//!
//! A row is `incoming` while an IPP Create-Job waits for its document, then `held`. Releasing
//! claims it (`releasing`, [`claim`]); the release marks it `released` with its ledger entry
//! ([`mark_released`]) in the transaction that inserts that entry, so once a job can reach CUPS
//! it can never be released again. A refusal puts it back ([`unclaim`]). The [`sweep`] removes
//! expired and old rows and returns stuck claims, but never a row that has a ledger entry.
//!
//! The documents themselves are files, see [`files`](mod@files). Only their owner reaches them:
//! administrators included, every query here is scoped to a user or to one of their device
//! passwords.

pub mod files;

use std::collections::HashSet;

use chrono::{DateTime, TimeDelta, Utc};
use recto_entity::{device_passwords, held_jobs, print_jobs};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, ConnectionTrait,
    DatabaseTransaction, DbErr, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionSession, TransactionTrait, sea_query::Expr,
};

use crate::{
    database::{begin_write, lock_user, locked},
    error::{Error, Problem, Result},
};

/// How many jobs a user may have waiting (incoming, held or being released).
pub const MAX_JOBS_PER_USER: u64 = 20;
/// How long an IPP Create-Job may wait for its document.
pub const INCOMING_LIFETIME: TimeDelta = TimeDelta::minutes(10);
/// How long a claim may last before the sweep returns the job, if it has no ledger entry: a
/// release that takes this long has crashed.
pub const CLAIM_LIFETIME: TimeDelta = TimeDelta::minutes(15);
/// How long a released row stays, for the device that sent it to see it printed.
pub const RELEASED_LIFETIME: TimeDelta = TimeDelta::days(1);

/// Where a held job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldState {
    /// Created by IPP Create-Job; its document has not arrived.
    Incoming,
    /// Waiting for its owner.
    Held,
    /// Claimed by one release.
    Releasing,
    /// Entered in the ledger and sent to CUPS, or on its way there.
    Released,
}

impl HeldState {
    /// The value of the `state` column.
    pub fn keyword(self) -> &'static str {
        match self {
            HeldState::Incoming => "incoming",
            HeldState::Held => "held",
            HeldState::Releasing => "releasing",
            HeldState::Released => "released",
        }
    }

    fn from_keyword(keyword: &str) -> Option<Self> {
        [
            HeldState::Incoming,
            HeldState::Held,
            HeldState::Releasing,
            HeldState::Released,
        ]
        .into_iter()
        .find(|state| state.keyword() == keyword)
    }
}

/// A held job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldJob {
    /// Also its IPP job id.
    pub id: i64,
    pub user_id: i64,
    /// The device password it came with; `None` once that is revoked.
    pub device_password_id: Option<i64>,
    pub state: HeldState,
    /// How many times it was claimed: a release holds the value its claim set.
    pub claim: i64,
    pub name: String,
    /// The MIME type of the file.
    pub stored_format: Option<String>,
    /// The MIME type the device sent, before any conversion.
    pub received_format: Option<String>,
    pub document_pages: Option<i64>,
    /// The size of the file, in bytes.
    pub size: i64,
    pub file: Option<String>,
    /// The options the device asked for, as JSON.
    pub requested: String,
    /// Why its last release was refused, as a refusal code.
    pub refusal: Option<String>,
    pub ledger_entry_id: Option<i64>,
    pub received_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// A document written to the [`files::HeldFiles`], to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub file: String,
    pub stored_format: String,
    pub received_format: Option<String>,
    pub document_pages: Option<i64>,
    pub size: i64,
}

/// A job received with its document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHeld {
    pub user_id: i64,
    pub device_password_id: Option<i64>,
    pub name: String,
    /// The options the device asked for, as JSON.
    pub requested: String,
    pub stored: Stored,
    pub now: DateTime<Utc>,
    /// How long it is kept (`RECTO_HELD_JOB_HOURS`).
    pub lifetime: TimeDelta,
}

/// What a deletion removed: the row, and the file it named, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    pub file: Option<String>,
}

/// What a [`sweep`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Swept {
    /// Held jobs removed because they expired.
    pub expired: u64,
    /// Create-Jobs removed because their document never came.
    pub abandoned: u64,
    /// Released jobs removed a day after their release.
    pub released: u64,
    /// The files those rows named, to remove.
    pub files: Vec<String>,
    /// Claims returned to `held`, from releases that never reached the ledger.
    pub returned: u64,
}

/// Whether the user has room for one more job of `incoming_bytes`: fewer than
/// [`MAX_JOBS_PER_USER`] waiting, and at most `max_bytes` in all. Checked before the document is
/// written, so a full queue is refused without touching the disk; [`receive`] and [`attach`]
/// check again.
pub async fn room(
    database: &impl ConnectionTrait,
    user_id: i64,
    incoming_bytes: i64,
    max_bytes: i64,
) -> Result<bool> {
    fits(database, user_id, None, incoming_bytes, max_bytes).await
}

/// Whether the user's `incoming` job `id` has room for its document of `incoming_bytes`: as
/// [`room`], counting that job once. Checked before the document is written; [`attach`] checks
/// again.
pub async fn room_for(
    database: &impl ConnectionTrait,
    user_id: i64,
    id: i64,
    incoming_bytes: i64,
    max_bytes: i64,
) -> Result<bool> {
    fits(database, user_id, Some(id), incoming_bytes, max_bytes).await
}

/// Records a job received with its document as `held`, unless the user has no [`room`] for it
/// ([`Problem::TooManyHeldJobs`]). Returns its id.
pub async fn receive(
    database: &(impl ConnectionTrait + TransactionTrait),
    job: NewHeld,
    max_bytes: i64,
) -> Result<i64> {
    let transaction = begin_write(database).await?;
    lock_user(&transaction, job.user_id).await?;
    if let Some(device) = job.device_password_id {
        owns_device(&transaction, job.user_id, device).await?;
    }
    if !fits(&transaction, job.user_id, None, job.stored.size, max_bytes).await? {
        return Err(too_many());
    }
    let row = held_jobs::ActiveModel {
        user_id: Set(job.user_id),
        device_password_id: Set(job.device_password_id),
        state: Set(HeldState::Held.keyword().to_owned()),
        claim: Set(0),
        name: Set(job.name),
        stored_format: Set(Some(job.stored.stored_format)),
        received_format: Set(job.stored.received_format),
        document_pages: Set(job.stored.document_pages),
        size: Set(job.stored.size),
        file: Set(Some(job.stored.file)),
        requested: Set(job.requested),
        refusal: Set(None),
        ledger_entry_id: Set(None),
        received_at: Set(job.now.fixed_offset()),
        expires_at: Set((job.now + job.lifetime).fixed_offset()),
        state_changed_at: Set(job.now.fixed_offset()),
        ..Default::default()
    }
    .insert(&transaction)
    .await?;
    transaction.commit().await?;
    Ok(row.id)
}

/// Records an IPP Create-Job: an `incoming` row waiting for its document, which [`attach`]
/// gives it. Refused with [`Problem::TooManyHeldJobs`] when the user's queue is full.
#[expect(clippy::too_many_arguments, reason = "the columns of a new row")]
pub async fn create_incoming(
    database: &(impl ConnectionTrait + TransactionTrait),
    user_id: i64,
    device_password_id: i64,
    name: &str,
    requested: &str,
    now: DateTime<Utc>,
    lifetime: TimeDelta,
    max_bytes: i64,
) -> Result<i64> {
    let transaction = begin_write(database).await?;
    lock_user(&transaction, user_id).await?;
    owns_device(&transaction, user_id, device_password_id).await?;
    if !fits(&transaction, user_id, None, 0, max_bytes).await? {
        return Err(too_many());
    }
    let row = held_jobs::ActiveModel {
        user_id: Set(user_id),
        device_password_id: Set(Some(device_password_id)),
        state: Set(HeldState::Incoming.keyword().to_owned()),
        claim: Set(0),
        name: Set(name.to_owned()),
        stored_format: Set(None),
        received_format: Set(None),
        document_pages: Set(None),
        size: Set(0),
        file: Set(None),
        requested: Set(requested.to_owned()),
        refusal: Set(None),
        ledger_entry_id: Set(None),
        received_at: Set(now.fixed_offset()),
        expires_at: Set((now + lifetime).fixed_offset()),
        state_changed_at: Set(now.fixed_offset()),
        ..Default::default()
    }
    .insert(&transaction)
    .await?;
    transaction.commit().await?;
    Ok(row.id)
}

/// Gives an `incoming` job of this device password its document, making it `held`. `false`
/// when the device has no such incoming job. Refused with [`Problem::TooManyHeldJobs`] when the
/// document does not fit the user's queue.
pub async fn attach(
    database: &(impl ConnectionTrait + TransactionTrait),
    device_password_id: i64,
    id: i64,
    stored: Stored,
    max_bytes: i64,
) -> Result<bool> {
    let transaction = begin_write(database).await?;
    let incoming = held_jobs::Entity::find_by_id(id)
        .filter(held_jobs::Column::DevicePasswordId.eq(device_password_id))
        .filter(held_jobs::Column::State.eq(HeldState::Incoming.keyword()))
        .one(&transaction)
        .await?;
    let Some(incoming) = incoming else {
        return Ok(false);
    };
    lock_user(&transaction, incoming.user_id).await?;
    // The job is one of the user's already: only its document is new.
    if !fits(
        &transaction,
        incoming.user_id,
        Some(id),
        stored.size,
        max_bytes,
    )
    .await?
    {
        return Err(too_many());
    }
    let updated = held_jobs::Entity::update_many()
        .col_expr(
            held_jobs::Column::State,
            Expr::value(HeldState::Held.keyword()),
        )
        .col_expr(held_jobs::Column::File, Expr::value(Some(stored.file)))
        .col_expr(
            held_jobs::Column::StoredFormat,
            Expr::value(Some(stored.stored_format)),
        )
        .col_expr(
            held_jobs::Column::ReceivedFormat,
            Expr::value(stored.received_format),
        )
        .col_expr(
            held_jobs::Column::DocumentPages,
            Expr::value(stored.document_pages),
        )
        .col_expr(held_jobs::Column::Size, Expr::value(stored.size))
        .filter(held_jobs::Column::Id.eq(id))
        .filter(held_jobs::Column::DevicePasswordId.eq(device_password_id))
        .filter(held_jobs::Column::State.eq(HeldState::Incoming.keyword()))
        .exec(&transaction)
        .await?;
    transaction.commit().await?;
    Ok(updated.rows_affected == 1)
}

/// The user's jobs waiting for them, `held` and `releasing`, newest first: the Jobs page.
pub async fn list(database: &impl ConnectionTrait, user_id: i64) -> Result<Vec<HeldJob>> {
    let rows = held_jobs::Entity::find()
        .filter(held_jobs::Column::UserId.eq(user_id))
        .filter(
            held_jobs::Column::State
                .is_in([HeldState::Held.keyword(), HeldState::Releasing.keyword()]),
        )
        .order_by_desc(held_jobs::Column::ReceivedAt)
        .order_by_desc(held_jobs::Column::Id)
        .all(database)
        .await?;
    rows.into_iter().map(job).collect()
}

/// Every job of a device password, whatever its state, newest first: what the device sees over
/// IPP.
pub async fn list_for_device(
    database: &impl ConnectionTrait,
    device_password_id: i64,
) -> Result<Vec<HeldJob>> {
    let rows = held_jobs::Entity::find()
        .filter(held_jobs::Column::DevicePasswordId.eq(device_password_id))
        .order_by_desc(held_jobs::Column::ReceivedAt)
        .order_by_desc(held_jobs::Column::Id)
        .all(database)
        .await?;
    rows.into_iter().map(job).collect()
}

/// The user's job `id`, if it is theirs. Administrators get only their own.
pub async fn get(
    database: &impl ConnectionTrait,
    user_id: i64,
    id: i64,
) -> Result<Option<HeldJob>> {
    let row = held_jobs::Entity::find_by_id(id)
        .filter(held_jobs::Column::UserId.eq(user_id))
        .one(database)
        .await?;
    row.map(job).transpose()
}

/// The job `id` of a device password, if it sent it.
pub async fn get_for_device(
    database: &impl ConnectionTrait,
    device_password_id: i64,
    id: i64,
) -> Result<Option<HeldJob>> {
    let row = held_jobs::Entity::find_by_id(id)
        .filter(held_jobs::Column::DevicePasswordId.eq(device_password_id))
        .one(database)
        .await?;
    row.map(job).transpose()
}

/// Claims the user's held job for one release: `held` to `releasing` in one conditional update,
/// so of two releases at once exactly one gets `Some`. An expired job is not claimed. The job
/// returned carries the claim the release must name to [`mark_released`] or [`unclaim`] it.
pub async fn claim(
    database: &impl ConnectionTrait,
    user_id: i64,
    id: i64,
    now: DateTime<Utc>,
) -> Result<Option<HeldJob>> {
    // An expired job waits only for the sweep.
    let row = held_jobs::Entity::find_by_id(id)
        .filter(held_jobs::Column::UserId.eq(user_id))
        .filter(held_jobs::Column::State.eq(HeldState::Held.keyword()))
        .filter(held_jobs::Column::ExpiresAt.gt(now.fixed_offset()))
        .one(database)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let claim = row.claim + 1;
    // Conditional on the state and the claim read: of two releases that read the same row, the
    // update of the second matches nothing.
    let updated = held_jobs::Entity::update_many()
        .col_expr(
            held_jobs::Column::State,
            Expr::value(HeldState::Releasing.keyword()),
        )
        .col_expr(held_jobs::Column::Claim, Expr::value(claim))
        .col_expr(
            held_jobs::Column::Refusal,
            Expr::value(Option::<String>::None),
        )
        .col_expr(
            held_jobs::Column::StateChangedAt,
            Expr::value(now.fixed_offset()),
        )
        .filter(held_jobs::Column::Id.eq(id))
        .filter(held_jobs::Column::State.eq(HeldState::Held.keyword()))
        .filter(held_jobs::Column::Claim.eq(row.claim))
        .exec(database)
        .await?;
    if updated.rows_affected != 1 {
        return Ok(None);
    }
    let mut claimed = job(row)?;
    claimed.state = HeldState::Releasing;
    claimed.claim = claim;
    claimed.refusal = None;
    Ok(Some(claimed))
}

/// Marks a claimed job `released` with its ledger entry. Call it in the transaction that inserts
/// the entry, so the two commit together. Fails with [`Error::NotFound`], to roll that
/// transaction back, unless the job is still under this claim: one the sweep returned to `held`
/// and another release claimed must not print twice.
pub async fn mark_released(
    transaction: &DatabaseTransaction,
    id: i64,
    claim: i64,
    ledger_entry_id: i64,
    now: DateTime<Utc>,
) -> Result<()> {
    let updated = held_jobs::Entity::update_many()
        .col_expr(
            held_jobs::Column::State,
            Expr::value(HeldState::Released.keyword()),
        )
        .col_expr(
            held_jobs::Column::LedgerEntryId,
            Expr::value(Some(ledger_entry_id)),
        )
        .col_expr(
            held_jobs::Column::StateChangedAt,
            Expr::value(now.fixed_offset()),
        )
        .filter(held_jobs::Column::Id.eq(id))
        .filter(held_jobs::Column::State.eq(HeldState::Releasing.keyword()))
        .filter(held_jobs::Column::Claim.eq(claim))
        .exec(transaction)
        .await?;
    if updated.rows_affected == 1 {
        Ok(())
    } else {
        Err(Error::NotFound)
    }
}

/// Puts a claimed job back to `held`, with why its release was refused: for a release refused
/// before it reached the ledger. Nothing happens unless the job is still `releasing` under this
/// claim: a `released` job has a ledger entry and goes back only with it refunded, through
/// [`return_refunded`].
pub async fn unclaim(
    database: &impl ConnectionTrait,
    id: i64,
    claim: i64,
    refusal: Option<&str>,
    now: DateTime<Utc>,
) -> Result<()> {
    held_jobs::Entity::update_many()
        .col_expr(
            held_jobs::Column::State,
            Expr::value(HeldState::Held.keyword()),
        )
        .col_expr(
            held_jobs::Column::Refusal,
            Expr::value(refusal.map(str::to_owned)),
        )
        .col_expr(
            held_jobs::Column::StateChangedAt,
            Expr::value(now.fixed_offset()),
        )
        .filter(held_jobs::Column::Id.eq(id))
        .filter(held_jobs::Column::Claim.eq(claim))
        .filter(held_jobs::Column::State.eq(HeldState::Releasing.keyword()))
        .exec(database)
        .await?;
    Ok(())
}

/// Refunds the ledger entry of a released job that CUPS refused, and puts the job back to
/// `held` with `refusal`, in one transaction: the entry and the job go back together or not at
/// all. Refused with [`Error::NotFound`], changing nothing, unless the job is `released` under
/// this claim with this entry.
pub async fn return_refunded(
    database: &(impl ConnectionTrait + TransactionTrait),
    id: i64,
    claim: i64,
    ledger_entry_id: i64,
    refusal: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let transaction = begin_write(database).await?;
    let returned = held_jobs::Entity::update_many()
        .col_expr(
            held_jobs::Column::State,
            Expr::value(HeldState::Held.keyword()),
        )
        .col_expr(
            held_jobs::Column::LedgerEntryId,
            Expr::value(Option::<i64>::None),
        )
        .col_expr(held_jobs::Column::Refusal, Expr::value(Some(refusal)))
        .col_expr(
            held_jobs::Column::StateChangedAt,
            Expr::value(now.fixed_offset()),
        )
        .filter(held_jobs::Column::Id.eq(id))
        .filter(held_jobs::Column::Claim.eq(claim))
        .filter(held_jobs::Column::State.eq(HeldState::Released.keyword()))
        .filter(held_jobs::Column::LedgerEntryId.eq(ledger_entry_id))
        .exec(&transaction)
        .await?;
    if returned.rows_affected != 1 {
        return Err(Error::NotFound);
    }
    crate::ledger::refund(&transaction, ledger_entry_id).await?;
    transaction.commit().await?;
    Ok(())
}

/// Deletes the user's job if it is `held`; a job being released stays. Returns what was
/// deleted, `None` when nothing was.
pub async fn delete(
    database: &(impl ConnectionTrait + TransactionTrait),
    user_id: i64,
    id: i64,
) -> Result<Option<Deleted>> {
    let condition = Condition::all()
        .add(held_jobs::Column::Id.eq(id))
        .add(held_jobs::Column::UserId.eq(user_id))
        .add(held_jobs::Column::State.eq(HeldState::Held.keyword()));
    delete_where(database, condition).await
}

/// Deletes a job of the device password still `incoming` (IPP Cancel-Job before its document).
/// A held job is reported completed to the device, so only its owner deletes it, with
/// [`delete`]. Returns what was deleted, `None` when nothing was.
pub async fn delete_for_device(
    database: &(impl ConnectionTrait + TransactionTrait),
    device_password_id: i64,
    id: i64,
) -> Result<Option<Deleted>> {
    let condition = Condition::all()
        .add(held_jobs::Column::Id.eq(id))
        .add(held_jobs::Column::DevicePasswordId.eq(device_password_id))
        .add(held_jobs::Column::State.eq(HeldState::Incoming.keyword()));
    delete_where(database, condition).await
}

/// How many jobs of the device password wait for their document, for the printer's
/// `queued-job-count`, which devices poll.
pub async fn incoming_count(
    database: &impl ConnectionTrait,
    device_password_id: i64,
) -> Result<u64> {
    Ok(held_jobs::Entity::find()
        .filter(held_jobs::Column::DevicePasswordId.eq(device_password_id))
        .filter(held_jobs::Column::State.eq(HeldState::Incoming.keyword()))
        .count(database)
        .await?)
}

/// Removes expired `held` rows, `incoming` rows older than [`INCOMING_LIFETIME`] or expired,
/// and `released` rows older than [`RELEASED_LIFETIME`]; returns `releasing` rows claimed more
/// than [`CLAIM_LIFETIME`] ago to `held` when they have no ledger entry. A row with a ledger
/// entry never goes back to `held`.
pub async fn sweep(
    database: &(impl ConnectionTrait + TransactionTrait),
    now: DateTime<Utc>,
) -> Result<Swept> {
    let expired = Condition::any()
        .add(
            Condition::all()
                .add(
                    held_jobs::Column::State
                        .is_in([HeldState::Held.keyword(), HeldState::Incoming.keyword()]),
                )
                .add(held_jobs::Column::ExpiresAt.lte(now.fixed_offset())),
        )
        .add(
            Condition::all()
                .add(held_jobs::Column::State.eq(HeldState::Incoming.keyword()))
                .add(held_jobs::Column::ReceivedAt.lte((now - INCOMING_LIFETIME).fixed_offset())),
        )
        .add(
            Condition::all()
                .add(held_jobs::Column::State.eq(HeldState::Released.keyword()))
                .add(
                    held_jobs::Column::StateChangedAt.lte((now - RELEASED_LIFETIME).fixed_offset()),
                ),
        );
    let transaction = begin_write(database).await?;
    // Locked on PostgreSQL, so a claim cannot take a row between the read and the delete: the
    // files returned are those of the rows deleted.
    let rows = locked(
        &transaction,
        held_jobs::Entity::find()
            .filter(expired.clone())
            .order_by_asc(held_jobs::Column::Id),
    )
    .all(&transaction)
    .await?;
    let mut swept = Swept::default();
    if !rows.is_empty() {
        let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
        // The rows are locked, so these are the rows deleted.
        held_jobs::Entity::delete_many()
            .filter(held_jobs::Column::Id.is_in(ids))
            .filter(expired)
            .exec(&transaction)
            .await?;
        for row in &rows {
            match HeldState::from_keyword(&row.state) {
                Some(HeldState::Incoming) => swept.abandoned += 1,
                Some(HeldState::Released) => swept.released += 1,
                _ => swept.expired += 1,
            }
        }
        swept.files = rows.into_iter().filter_map(|row| row.file).collect();
    }
    // A row with a ledger entry may have reached CUPS: it never goes back.
    swept.returned = held_jobs::Entity::update_many()
        .col_expr(
            held_jobs::Column::State,
            Expr::value(HeldState::Held.keyword()),
        )
        .col_expr(
            held_jobs::Column::StateChangedAt,
            Expr::value(now.fixed_offset()),
        )
        .filter(held_jobs::Column::State.eq(HeldState::Releasing.keyword()))
        .filter(held_jobs::Column::StateChangedAt.lte((now - CLAIM_LIFETIME).fixed_offset()))
        .filter(held_jobs::Column::LedgerEntryId.is_null())
        .exec(&transaction)
        .await?
        .rows_affected;
    transaction.commit().await?;
    Ok(swept)
}

/// Released jobs whose ledger entry never got a CUPS job id within
/// [`ATTACH_GRACE`](crate::ledger::ATTACH_GRACE): CUPS refused them and returning them failed,
/// or recto stopped before sending them. The caller returns with [`return_refunded`] those
/// whose document is still there: a document CUPS took is removed at once, so a job whose file
/// remains never reached it.
pub async fn unsent(database: &impl ConnectionTrait, now: DateTime<Utc>) -> Result<Vec<HeldJob>> {
    let entries: Vec<i64> = print_jobs::Entity::find()
        .select_only()
        .column(print_jobs::Column::Id)
        .filter(print_jobs::Column::CupsJobId.is_null())
        .filter(
            print_jobs::Column::SubmittedAt.lt((now - crate::ledger::ATTACH_GRACE).fixed_offset()),
        )
        .filter(
            print_jobs::Column::Id.in_subquery(
                sea_orm::sea_query::Query::select()
                    .column(held_jobs::Column::LedgerEntryId)
                    .from(held_jobs::Entity)
                    .and_where(held_jobs::Column::State.eq(HeldState::Released.keyword()))
                    .to_owned(),
            ),
        )
        .into_tuple()
        .all(database)
        .await?;
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let rows = held_jobs::Entity::find()
        .filter(held_jobs::Column::State.eq(HeldState::Released.keyword()))
        .filter(held_jobs::Column::LedgerEntryId.is_in(entries))
        .filter(held_jobs::Column::File.is_not_null())
        .order_by_asc(held_jobs::Column::Id)
        .all(database)
        .await?;
    rows.into_iter().map(job).collect()
}

/// Every file a row names, for the orphan sweep.
pub async fn files(database: &impl ConnectionTrait) -> Result<HashSet<String>> {
    let files: Vec<Option<String>> = held_jobs::Entity::find()
        .select_only()
        .column(held_jobs::Column::File)
        .filter(held_jobs::Column::File.is_not_null())
        .into_tuple()
        .all(database)
        .await?;
    Ok(files.into_iter().flatten().collect())
}

/// The states of a job that counts towards the user's limits.
const WAITING: [HeldState; 3] = [HeldState::Incoming, HeldState::Held, HeldState::Releasing];

fn too_many() -> Error {
    Error::Invalid(Problem::TooManyHeldJobs {
        maximum: MAX_JOBS_PER_USER,
    })
}

/// Whether the user's waiting jobs, but `except`, leave room for one more of `incoming_bytes`.
async fn fits(
    database: &impl ConnectionTrait,
    user_id: i64,
    except: Option<i64>,
    incoming_bytes: i64,
    max_bytes: i64,
) -> Result<bool> {
    let mut query = held_jobs::Entity::find()
        .select_only()
        .column(held_jobs::Column::Size)
        .filter(held_jobs::Column::UserId.eq(user_id))
        .filter(held_jobs::Column::State.is_in(WAITING.map(HeldState::keyword)));
    if let Some(id) = except {
        query = query.filter(held_jobs::Column::Id.ne(id));
    }
    // Summed here: at most a few dozen rows, and PostgreSQL's sum of big integers is a numeric.
    let sizes: Vec<i64> = query.into_tuple().all(database).await?;
    let total = sizes.iter().sum::<i64>().saturating_add(incoming_bytes);
    Ok((sizes.len() as u64) < MAX_JOBS_PER_USER && total <= max_bytes)
}

/// Refuses with [`Error::NotFound`] a device password that is not the user's: its jobs would
/// show to another person's device.
async fn owns_device(
    transaction: &impl ConnectionTrait,
    user_id: i64,
    device_password_id: i64,
) -> Result<()> {
    let owned = device_passwords::Entity::find_by_id(device_password_id)
        .filter(device_passwords::Column::UserId.eq(user_id))
        .count(transaction)
        .await?;
    if owned == 1 {
        Ok(())
    } else {
        Err(Error::NotFound)
    }
}

/// Deletes the row matching `condition`, if any, and returns the file it named.
async fn delete_where(
    database: &(impl ConnectionTrait + TransactionTrait),
    condition: Condition,
) -> Result<Option<Deleted>> {
    let transaction = begin_write(database).await?;
    // Read under the lock, so the file returned is the one of the row deleted, even if its
    // document was attached meanwhile.
    let row = locked(
        &transaction,
        held_jobs::Entity::find().filter(condition.clone()),
    )
    .one(&transaction)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let deleted = held_jobs::Entity::delete_many()
        .filter(held_jobs::Column::Id.eq(row.id))
        .filter(condition)
        .exec(&transaction)
        .await?;
    transaction.commit().await?;
    Ok((deleted.rows_affected == 1).then_some(Deleted { file: row.file }))
}

fn job(row: held_jobs::Model) -> Result<HeldJob> {
    let state = HeldState::from_keyword(&row.state).ok_or_else(|| {
        Error::Database(DbErr::Custom(format!(
            "unknown held job state {:?}",
            row.state
        )))
    })?;
    Ok(HeldJob {
        id: row.id,
        user_id: row.user_id,
        device_password_id: row.device_password_id,
        state,
        claim: row.claim,
        name: row.name,
        stored_format: row.stored_format,
        received_format: row.received_format,
        document_pages: row.document_pages,
        size: row.size,
        file: row.file,
        requested: row.requested,
        refusal: row.refusal,
        ledger_entry_id: row.ledger_entry_id,
        received_at: row.received_at.with_timezone(&Utc),
        expires_at: row.expires_at.with_timezone(&Utc),
    })
}

#[cfg(test)]
mod tests;
