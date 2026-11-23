//! User accounts and how people sign in to them.
//!
//! People sign in through OpenID Connect ([`sign_in_with_identity`]), with a local password
//! ([`sign_in_with_password`]), or both. There is no self sign-up: local accounts are created by
//! administrators ([`create_local`]), single sign-on accounts at their first sign-in. A local
//! account and a single sign-on identity are different users, even with the same email address.
//!
//! Functions that act on another user take the acting user's id only to refuse changes to their
//! own account; checking that the actor is an administrator is the caller's job.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DurationRound, TimeDelta, Utc};
use recto_entity::{group_memberships, groups, user_identities, users};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait,
    QueryFilter, QueryOrder, SqlErr, TransactionSession, TransactionTrait,
    prelude::DateTimeWithTimeZone, sea_query::Expr,
};

use crate::{
    database::begin_write,
    device_passwords,
    error::{Error, Problem, Result},
    names,
    passwords::{self, PasswordPolicy},
};

/// Who an identity provider says signed in, read from its claims.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    /// Whether the administrator claim grants administration; `None` when no such claim is
    /// configured, which leaves administration as it is. The claim grants administration to a
    /// user who does not have it and takes back only what it granted: an administrator appointed
    /// by hand stays one whatever the claim says.
    pub administrator: Option<bool>,
    /// The groups claim; `None` when no such claim is configured or its value is unknown, which
    /// leaves memberships as they are.
    pub groups: Option<Vec<String>>,
}

/// A local account to create.
#[derive(Clone)]
pub struct NewLocalUser {
    /// Must follow [`names::is_valid_local_username`].
    pub username: String,
    /// Defaults to the username when blank.
    pub display_name: String,
    pub email: Option<String>,
    /// The plain password; it is hashed before it is stored.
    pub password: String,
    pub is_administrator: bool,
}

/// A user as the Users page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    pub user: users::Model,
    /// Names of the user's groups, in alphabetical order.
    pub groups: Vec<String>,
    /// Ids of the user's groups, in no particular order.
    pub group_ids: Vec<i64>,
    pub has_password: bool,
    pub has_identity: bool,
}

/// Signs in the person an identity provider vouches for: finds the user by issuer and subject
/// (never by username or email, which the provider may let people change), or creates one.
///
/// A new user's username is the first non-empty of `preferred_username`, `email` and `subject`,
/// with `-2`, `-3`… on a clash, and never changes afterwards. Each sign-in refreshes the display
/// name (`name`, else the username), the email address (unless another user has it) and the
/// last sign-in time, then applies the administrator and groups claims when they are known
/// (see [`Identity::administrator`] for how the claim and manual grants combine).
///
/// Two first sign-ins of the same person can race; the loser hits a unique index and the whole
/// sign-in is retried once, which then finds the winner's user. Only once: when three or more
/// race, or the retry meets yet another clash, one of them can still fail with
/// [`Error::Database`], and that person signs in again.
pub async fn sign_in_with_identity(
    database: &(impl ConnectionTrait + TransactionTrait),
    identity: &Identity,
) -> Result<users::Model> {
    match sign_in_with_identity_once(database, identity).await {
        Err(Error::Database(error)) if is_unique_violation(&error) => {
            sign_in_with_identity_once(database, identity).await
        }
        result => result,
    }
}

async fn sign_in_with_identity_once(
    database: &(impl ConnectionTrait + TransactionTrait),
    identity: &Identity,
) -> Result<users::Model> {
    let transaction = begin_write(database).await?;
    let user = match find_by_identity(&transaction, &identity.issuer, &identity.subject).await? {
        Some(user) if user.is_disabled => return Err(Error::Disabled),
        Some(user) => user,
        None => create_for_identity(&transaction, identity).await?,
    };

    let mut changes: users::ActiveModel = user.clone().into();
    changes.display_name = Set(non_empty(identity.name.as_deref())
        .unwrap_or(&user.username)
        .to_owned());
    // A provider that stops sending the email address leaves the known one in place.
    if let Some(email) = normalize_email(identity.email.as_deref())
        && user.email.as_ref() != Some(&email)
        && !email_taken(&transaction, &email, Some(user.id)).await?
    {
        changes.email = Set(Some(email));
    }
    match identity.administrator {
        // A manual grant stays manual even when the claim agrees, so that a later `false` from
        // the claim does not take it back.
        Some(true) if !user.is_administrator => {
            changes.is_administrator = Set(true);
            changes.administrator_from_claim = Set(true);
        }
        // Only what the claim granted is taken back: an administrator appointed by hand stays.
        Some(false) if user.administrator_from_claim => {
            changes.is_administrator = Set(false);
            changes.administrator_from_claim = Set(false);
        }
        Some(_) | None => {}
    }
    changes.last_sign_in_at = Set(Some(Utc::now().fixed_offset()));
    let user = changes.update(&transaction).await?;

    if let Some(groups) = &identity.groups {
        replace_memberships(&transaction, user.id, groups).await?;
    }
    transaction.commit().await?;
    Ok(user)
}

/// The user linked to an identity provider account.
async fn find_by_identity(
    database: &impl ConnectionTrait,
    issuer: &str,
    subject: &str,
) -> Result<Option<users::Model>> {
    let identity = user_identities::Entity::find()
        .filter(user_identities::Column::Issuer.eq(issuer))
        .filter(user_identities::Column::Subject.eq(subject))
        .one(database)
        .await?;
    match identity {
        Some(identity) => find_by_id(database, identity.user_id).await,
        None => Ok(None),
    }
}

/// Creates the user of an identity's first sign-in, with its username and identity; the caller
/// fills in the rest of the profile.
async fn create_for_identity(
    database: &impl ConnectionTrait,
    identity: &Identity,
) -> Result<users::Model> {
    // Unlike nanobom, the name is not lower-cased or cleaned up: it is the job owner CUPS
    // records, and v1 already submitted jobs under the name exactly as the provider sent it.
    let base = non_empty(identity.preferred_username.as_deref())
        .or(non_empty(identity.email.as_deref()))
        .unwrap_or(&identity.subject);
    let username = available_username(database, base).await?;
    let now = Utc::now().fixed_offset();
    let user = users::ActiveModel {
        username: Set(username.clone()),
        display_name: Set(username),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(database)
    .await?;
    user_identities::ActiveModel {
        user_id: Set(user.id),
        issuer: Set(identity.issuer.clone()),
        subject: Set(identity.subject.clone()),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(database)
    .await?;
    tracing::info!(user = %user.username, "user created at first sign-in");
    Ok(user)
}

/// The first free username among `base`, `base-2`, `base-3`… A concurrent sign-in can still
/// take it first; the unique index then fails the insert and the sign-in is retried.
async fn available_username(database: &impl ConnectionTrait, base: &str) -> Result<String> {
    for suffix in 1.. {
        let candidate = if suffix == 1 {
            base.to_owned()
        } else {
            format!("{base}-{suffix}")
        };
        if find_by_username(database, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    unreachable!("an unbounded range always yields a free username")
}

/// Makes the user a member of exactly these groups, creating the ones not seen before. Groups
/// left without members are kept: they are never deleted automatically.
///
/// It runs at every single sign-on, where the groups rarely change: when they have not, the
/// user's groups are the only query.
async fn replace_memberships(
    database: &impl ConnectionTrait,
    user_id: i64,
    names: &[String],
) -> Result<()> {
    let names: BTreeSet<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| !name.is_empty())
        .collect();
    let current: HashMap<String, i64> = groups::Entity::find()
        .inner_join(group_memberships::Entity)
        .filter(group_memberships::Column::UserId.eq(user_id))
        .all(database)
        .await?
        .into_iter()
        .map(|group| (group.name, group.id))
        .collect();
    if current.len() == names.len() && names.iter().all(|name| current.contains_key(*name)) {
        return Ok(());
    }

    let joined: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| !current.contains_key(*name))
        .collect();
    let mut known: HashMap<String, i64> = if joined.is_empty() {
        HashMap::new()
    } else {
        groups::Entity::find()
            .filter(groups::Column::Name.is_in(joined.iter().copied()))
            .all(database)
            .await?
            .into_iter()
            .map(|group| (group.name, group.id))
            .collect()
    };
    let mut added = Vec::new();
    for name in joined {
        let id = match known.remove(name) {
            Some(id) => id,
            None => {
                groups::ActiveModel {
                    name: Set(name.to_owned()),
                    created_at: Set(Utc::now().fixed_offset()),
                    ..Default::default()
                }
                .insert(database)
                .await?
                .id
            }
        };
        added.push(id);
    }

    let left: Vec<i64> = current
        .iter()
        .filter(|(name, _)| !names.contains(name.as_str()))
        .map(|(_, id)| *id)
        .collect();
    if !left.is_empty() {
        group_memberships::Entity::delete_many()
            .filter(group_memberships::Column::UserId.eq(user_id))
            .filter(group_memberships::Column::GroupId.is_in(left))
            .exec(database)
            .await?;
    }
    if !added.is_empty() {
        group_memberships::Entity::insert_many(added.into_iter().map(|group_id| {
            group_memberships::ActiveModel {
                group_id: Set(group_id),
                user_id: Set(user_id),
                ..Default::default()
            }
        }))
        .exec(database)
        .await?;
    }
    Ok(())
}

/// Signs in with a username or an email address and a password.
///
/// Every failure is [`Error::InvalidCredentials`]: unknown login, wrong password, an account
/// without a password, or a disabled account. A password is hashed in every case, against a
/// fixed dummy hash when there is no real one, so the response time does not tell them apart
/// either. A password longer than [`passwords::MAXIMUM_LENGTH`] cannot be right and is refused
/// without hashing it, so long inputs cannot be used to exhaust the server.
pub async fn sign_in_with_password(
    database: &(impl ConnectionTrait + TransactionTrait),
    login: &str,
    password: String,
) -> Result<users::Model> {
    if password.chars().count() > passwords::MAXIMUM_LENGTH {
        return Err(Error::InvalidCredentials);
    }
    let user = find_by_login(database, login).await?;
    let matches = match user.as_ref().and_then(|user| user.password_hash.clone()) {
        Some(hash) => passwords::verify(password, hash).await,
        None => {
            passwords::verify_without_hash(password).await;
            false
        }
    };
    // A disabled account answers like a wrong password, so the form never confirms that a
    // password is right.
    let user = match user {
        Some(user) if matches && !user.is_disabled => user,
        _ => return Err(Error::InvalidCredentials),
    };
    let mut changes: users::ActiveModel = user.into();
    changes.last_sign_in_at = Set(Some(Utc::now().fixed_offset()));
    Ok(changes.update(database).await?)
}

/// The user a login names: a username (exactly as stored), else an email address (in any
/// case). Usernames come first because single sign-on usernames may look like email addresses.
async fn find_by_login(
    database: &impl ConnectionTrait,
    login: &str,
) -> Result<Option<users::Model>> {
    if let Some(user) = find_by_username(database, login).await? {
        return Ok(Some(user));
    }
    match normalize_email(Some(login)) {
        Some(email) => Ok(users::Entity::find()
            .filter(users::Column::Email.eq(email))
            .one(database)
            .await?),
        None => Ok(None),
    }
}

/// Creates a local account, for an administrator. Refuses an invalid or taken username, a taken
/// email address and a password outside the policy with [`Error::Invalid`].
pub async fn create_local(
    database: &(impl ConnectionTrait + TransactionTrait),
    user: NewLocalUser,
    policy: &PasswordPolicy,
) -> Result<users::Model> {
    if !names::is_valid_local_username(&user.username) {
        return Err(Error::Invalid(Problem::UsernameInvalid));
    }
    policy.validate(&user.password)?;
    let email = normalize_email(user.email.as_deref());
    let display_name = non_empty(Some(user.display_name.trim()))
        .unwrap_or(&user.username)
        .to_owned();
    // Hashing is slow: do it before taking the write lock.
    let password_hash = passwords::hash(user.password).await;

    let transaction = begin_write(database).await?;
    if find_by_username(&transaction, &user.username)
        .await?
        .is_some()
    {
        return Err(Error::Invalid(Problem::UsernameTaken));
    }
    if let Some(email) = &email
        && email_taken(&transaction, email, None).await?
    {
        return Err(Error::Invalid(Problem::EmailTaken));
    }
    let created = users::ActiveModel {
        username: Set(user.username),
        display_name: Set(display_name),
        email: Set(email),
        password_hash: Set(Some(password_hash)),
        is_administrator: Set(user.is_administrator),
        created_at: Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(&transaction)
    .await
    .map_err(taken)?;
    transaction.commit().await?;
    tracing::info!(user = %created.username, "local user created");
    Ok(created)
}

/// Sets a user's password, for an administrator resetting it or a user changing their own (the
/// caller checks the current password). It also lets a single sign-on user sign in locally.
///
/// It ends every session of the user by setting `sessions_valid_after` to now, rounded up to
/// the next whole microsecond. The rule sessions are checked against: a session is valid only if
/// its sign-in time is at or after `sessions_valid_after`. Any session signed in before the
/// call, even within the same second, is earlier, so it is rejected; the sign-in time must be
/// kept to at least the microsecond for that to hold.
pub async fn set_password(
    database: &(impl ConnectionTrait + TransactionTrait),
    user_id: i64,
    password: String,
    policy: &PasswordPolicy,
) -> Result<()> {
    policy.validate(&password)?;
    let user = find_by_id(database, user_id)
        .await?
        .ok_or(Error::NotFound)?;
    let password_hash = passwords::hash(password).await;
    let mut changes: users::ActiveModel = user.into();
    changes.password_hash = Set(Some(password_hash));
    changes.sessions_valid_after = Set(Some(sessions_cutoff()));
    changes.update(database).await?;
    Ok(())
}

/// [`set_password`] for an administrator resetting another user's password, which also
/// revokes all their device passwords: a reset is what follows a compromised account, and a
/// device password the intruder made would otherwise outlive it. Both happen in one
/// transaction. Returns how many device passwords were revoked.
pub async fn reset_password(
    database: &(impl ConnectionTrait + TransactionTrait),
    user_id: i64,
    password: String,
    policy: &PasswordPolicy,
) -> Result<u64> {
    policy.validate(&password)?;
    let password_hash = passwords::hash(password).await;
    let transaction = begin_write(database).await?;
    let user = find_by_id(&transaction, user_id)
        .await?
        .ok_or(Error::NotFound)?;
    let mut changes: users::ActiveModel = user.into();
    changes.password_hash = Set(Some(password_hash));
    changes.sessions_valid_after = Set(Some(sessions_cutoff()));
    changes.update(&transaction).await?;
    let revoked = device_passwords::revoke_all(&transaction, user_id).await?;
    transaction.commit().await?;
    Ok(revoked)
}

/// Disables or re-enables a user. Disabling ends every session of the user, as
/// [`set_password`] does; re-enabling does not bring them back. Refused on the actor's own
/// account, so the last administrator cannot lock everyone out.
pub async fn set_disabled(
    database: &(impl ConnectionTrait + TransactionTrait),
    actor_id: i64,
    user_id: i64,
    disabled: bool,
) -> Result<()> {
    let user = other_user(database, actor_id, user_id).await?;
    let mut changes: users::ActiveModel = user.into();
    changes.is_disabled = Set(disabled);
    if disabled {
        changes.sessions_valid_after = Set(Some(sessions_cutoff()));
    }
    changes.update(database).await?;
    Ok(())
}

/// Sets the printer the user's computers and phones print to directly; `None`, or a blank name,
/// holds their documents until the user releases them. Whether the user may use the printer is
/// the caller's to check, and checked again at each job.
pub async fn set_direct_printer(
    database: &impl ConnectionTrait,
    user_id: i64,
    printer: Option<&str>,
) -> Result<()> {
    let printer = printer
        .map(str::trim)
        .filter(|printer| !printer.is_empty())
        .map(str::to_owned);
    let updated = users::Entity::update_many()
        .col_expr(users::Column::DirectPrinter, Expr::value(printer))
        .filter(users::Column::Id.eq(user_id))
        .exec(database)
        .await?;
    if updated.rows_affected == 1 {
        Ok(())
    } else {
        Err(Error::NotFound)
    }
}

/// Grants or removes administration by hand. The grant is marked as manual, so a later
/// administrator claim of `false` does not take it back. Refused on the actor's own account, and
/// on administration the claim granted ([`Problem::AdministratorFromClaim`]): removing it would
/// last until the next sign-in, and granting it again would quietly take it from the provider.
pub async fn set_administrator(
    database: &(impl ConnectionTrait + TransactionTrait),
    actor_id: i64,
    user_id: i64,
    administrator: bool,
) -> Result<()> {
    let user = other_user(database, actor_id, user_id).await?;
    if user.administrator_from_claim {
        return Err(Error::Invalid(Problem::AdministratorFromClaim));
    }
    store_administrator(database, user, administrator).await
}

/// Like [`set_administrator`], for the command line. It has no actor, so nothing is refused: the
/// operator can reach the database anyway, and this is how the last administrator is replaced.
pub async fn set_administrator_from_command_line(
    database: &(impl ConnectionTrait + TransactionTrait),
    user_id: i64,
    administrator: bool,
) -> Result<()> {
    let user = find_by_id(database, user_id)
        .await?
        .ok_or(Error::NotFound)?;
    store_administrator(database, user, administrator).await
}

async fn store_administrator(
    database: &impl ConnectionTrait,
    user: users::Model,
    administrator: bool,
) -> Result<()> {
    let mut changes: users::ActiveModel = user.into();
    changes.is_administrator = Set(administrator);
    changes.administrator_from_claim = Set(false);
    changes.update(database).await?;
    Ok(())
}

async fn other_user(
    database: &impl ConnectionTrait,
    actor_id: i64,
    user_id: i64,
) -> Result<users::Model> {
    if actor_id == user_id {
        return Err(Error::Invalid(Problem::OwnAccount));
    }
    find_by_id(database, user_id).await?.ok_or(Error::NotFound)
}

/// The value of `sessions_valid_after` that ends every session signed in so far (the rule is
/// on [`set_password`]).
///
/// Now is rounded up to the next whole microsecond, the finest precision PostgreSQL keeps, so
/// the stored value stays strictly later than any sign-in before the call, whatever the
/// database. A sign-in within that microsecond after the call may be rejected too; that person
/// signs in again.
fn sessions_cutoff() -> DateTimeWithTimeZone {
    let microsecond = TimeDelta::microseconds(1);
    let now = Utc::now();
    // Truncating to a whole microsecond cannot fail for present-day times.
    let truncated = now.duration_trunc(microsecond).unwrap_or(now);
    (truncated + microsecond).fixed_offset()
}

/// Whether any user, disabled or not, has a password. Until one does, nobody can sign in with
/// a password, so the sign-in page may send everyone to single sign-on.
pub async fn any_with_password(database: &impl ConnectionTrait) -> Result<bool> {
    Ok(users::Entity::find()
        .filter(users::Column::PasswordHash.is_not_null())
        .one(database)
        .await?
        .is_some())
}

/// Finds a user by identifier, disabled users included.
pub async fn find_by_id(database: &impl ConnectionTrait, id: i64) -> Result<Option<users::Model>> {
    Ok(users::Entity::find_by_id(id).one(database).await?)
}

/// Finds a user by username, compared exactly (single sign-on usernames keep their case);
/// disabled users included.
pub async fn find_by_username(
    database: &impl ConnectionTrait,
    username: &str,
) -> Result<Option<users::Model>> {
    Ok(users::Entity::find()
        .filter(users::Column::Username.eq(username))
        .one(database)
        .await?)
}

/// Every user, by username, with their groups and the ways they can sign in.
pub async fn list(database: &impl ConnectionTrait) -> Result<Vec<UserSummary>> {
    let users = users::Entity::find()
        .order_by_asc(users::Column::Username)
        .all(database)
        .await?;
    let group_names: HashMap<i64, String> = groups::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|group| (group.id, group.name))
        .collect();
    let mut groups_of: HashMap<i64, (Vec<String>, Vec<i64>)> = HashMap::new();
    for membership in group_memberships::Entity::find().all(database).await? {
        if let Some(name) = group_names.get(&membership.group_id) {
            let (names, ids) = groups_of.entry(membership.user_id).or_default();
            names.push(name.clone());
            ids.push(membership.group_id);
        }
    }
    let with_identity: HashSet<i64> = user_identities::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|identity| identity.user_id)
        .collect();
    Ok(users
        .into_iter()
        .map(|user| {
            let (mut groups, group_ids) = groups_of.remove(&user.id).unwrap_or_default();
            groups.sort();
            UserSummary {
                groups,
                group_ids,
                has_password: user.password_hash.is_some(),
                has_identity: with_identity.contains(&user.id),
                user,
            }
        })
        .collect())
}

/// Whether a user other than `except` has this email address.
async fn email_taken(
    database: &impl ConnectionTrait,
    email: &str,
    except: Option<i64>,
) -> Result<bool> {
    let mut query = users::Entity::find().filter(users::Column::Email.eq(email));
    if let Some(id) = except {
        query = query.filter(users::Column::Id.ne(id));
    }
    Ok(query.one(database).await?.is_some())
}

/// Email addresses are stored trimmed and in lower case, so signing in with one, and the
/// uniqueness check, ignore case.
fn normalize_email(email: Option<&str>) -> Option<String> {
    non_empty(email.map(str::trim)).map(str::to_lowercase)
}

/// The value unless it is missing or empty, as v1 read the claims.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn is_unique_violation(error: &DbErr) -> bool {
    matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

/// The problem of a user insert that lost a race on the username or the email address. On
/// PostgreSQL the checks before it do not lock anything, so a concurrent creation (or a first
/// single sign-on picking the same username) can pass them too; the unique index then refuses
/// the second. Its message names the column (`users.email`, `users_email_key`).
fn taken(error: DbErr) -> Error {
    match error.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(message)) if message.contains("email") => {
            Error::Invalid(Problem::EmailTaken)
        }
        Some(SqlErr::UniqueConstraintViolation(_)) => Error::Invalid(Problem::UsernameTaken),
        _ => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use recto_entity::{group_memberships, groups};
    use sea_orm::{DatabaseConnection, EntityTrait, PaginatorTrait};

    use super::*;
    use crate::error::{Error, Problem};

    const POLICY: PasswordPolicy = PasswordPolicy { minimum_length: 12 };
    const PASSWORD: &str = "correct horse battery staple";

    fn identity(subject: &str, preferred_username: &str) -> Identity {
        Identity {
            issuer: "https://id.example".to_owned(),
            subject: subject.to_owned(),
            preferred_username: Some(preferred_username.to_owned()),
            ..Identity::default()
        }
    }

    fn local(username: &str, email: Option<&str>) -> NewLocalUser {
        NewLocalUser {
            username: username.to_owned(),
            display_name: username.to_owned(),
            email: email.map(str::to_owned),
            password: PASSWORD.to_owned(),
            is_administrator: false,
        }
    }

    async fn user_count(database: &DatabaseConnection) -> u64 {
        users::Entity::find().count(database).await.unwrap()
    }

    async fn reload(database: &DatabaseConnection, id: i64) -> users::Model {
        find_by_id(database, id)
            .await
            .unwrap()
            .expect("the user exists")
    }

    async fn group_names(database: &DatabaseConnection, user_id: i64) -> Vec<String> {
        let summaries = list(database).await.unwrap();
        summaries
            .into_iter()
            .find(|summary| summary.user.id == user_id)
            .expect("the user is listed")
            .groups
    }

    #[tokio::test]
    async fn the_direct_printer_is_set_and_cleared() {
        let database = crate::testing::database().await;
        let alice = create_local(&database, local("alice", None), &POLICY)
            .await
            .unwrap();
        assert_eq!(reload(&database, alice.id).await.direct_printer, None);

        set_direct_printer(&database, alice.id, Some("office"))
            .await
            .unwrap();
        assert_eq!(
            reload(&database, alice.id).await.direct_printer.as_deref(),
            Some("office")
        );
        // Blank holds, as no printer does.
        set_direct_printer(&database, alice.id, Some("  "))
            .await
            .unwrap();
        assert_eq!(reload(&database, alice.id).await.direct_printer, None);
        set_direct_printer(&database, alice.id, Some("office"))
            .await
            .unwrap();
        set_direct_printer(&database, alice.id, None).await.unwrap();
        assert_eq!(reload(&database, alice.id).await.direct_printer, None);

        assert!(matches!(
            set_direct_printer(&database, alice.id + 1, Some("office")).await,
            Err(Error::NotFound)
        ));
    }

    #[tokio::test]
    async fn identity_creates_then_finds_the_same_user() {
        let database = crate::testing::database().await;
        let alice = identity("alice-subject", "alice");

        let first = sign_in_with_identity(&database, &alice).await.unwrap();
        let second = sign_in_with_identity(&database, &alice).await.unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(user_count(&database).await, 1);
        assert!(second.last_sign_in_at.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_first_sign_ins_create_one_user() {
        let database = crate::testing::database().await;
        let alice = identity("alice-subject", "alice");
        // Two open connections, so neither sign-in waits for the pool to connect and both
        // start their transactions together.
        let warm = (
            database.begin().await.unwrap(),
            database.begin().await.unwrap(),
        );
        drop(warm);
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));

        let sign_in = || {
            let (database, alice, start) = (database.clone(), alice.clone(), start.clone());
            tokio::spawn(async move {
                start.wait().await;
                sign_in_with_identity(&database, &alice).await
            })
        };
        let (first, second) = (sign_in(), sign_in());
        let first = first.await.unwrap().expect("the first sign-in succeeds");
        let second = second.await.unwrap().expect("the second sign-in succeeds");

        assert_eq!(first.id, second.id);
        assert_eq!(user_count(&database).await, 1);
    }

    #[tokio::test]
    async fn username_is_kept_as_sent_and_never_changes() {
        let database = crate::testing::database().await;

        let created = sign_in_with_identity(&database, &identity("s", "Alice.Martin@Corp"))
            .await
            .unwrap();
        let again = sign_in_with_identity(&database, &identity("s", "alice"))
            .await
            .unwrap();

        assert_eq!(created.username, "Alice.Martin@Corp");
        assert_eq!(again.username, "Alice.Martin@Corp");
    }

    #[tokio::test]
    async fn username_falls_back_to_email_then_subject() {
        let database = crate::testing::database().await;
        let by_email = Identity {
            issuer: "https://id.example".to_owned(),
            subject: "one".to_owned(),
            preferred_username: Some(String::new()),
            email: Some("Bob@Example.org".to_owned()),
            ..Identity::default()
        };
        let by_subject = Identity {
            issuer: "https://id.example".to_owned(),
            subject: "two".to_owned(),
            ..Identity::default()
        };

        let bob = sign_in_with_identity(&database, &by_email).await.unwrap();
        let anonymous = sign_in_with_identity(&database, &by_subject).await.unwrap();

        assert_eq!(bob.username, "Bob@Example.org");
        assert_eq!(bob.display_name, "Bob@Example.org");
        assert_eq!(anonymous.username, "two");
    }

    #[tokio::test]
    async fn username_clash_gets_a_suffix() {
        let database = crate::testing::database().await;

        let first = sign_in_with_identity(&database, &identity("one", "alice"))
            .await
            .unwrap();
        let second = sign_in_with_identity(&database, &identity("two", "alice"))
            .await
            .unwrap();
        let third = sign_in_with_identity(&database, &identity("three", "alice"))
            .await
            .unwrap();

        assert_eq!(first.username, "alice");
        assert_eq!(second.username, "alice-2");
        assert_eq!(third.username, "alice-3");
    }

    #[tokio::test]
    async fn profile_is_refreshed_and_email_kept_unique() {
        let database = crate::testing::database().await;
        create_local(
            &database,
            local("carol", Some("carol@example.org")),
            &POLICY,
        )
        .await
        .unwrap();
        let mut alice = identity("s", "alice");
        alice.name = Some("Alice Martin".to_owned());
        alice.email = Some("Alice@Example.org".to_owned());

        let created = sign_in_with_identity(&database, &alice).await.unwrap();
        assert_eq!(created.display_name, "Alice Martin");
        assert_eq!(created.email.as_deref(), Some("alice@example.org"));

        alice.name = None;
        alice.email = Some("carol@example.org".to_owned());
        let refreshed = sign_in_with_identity(&database, &alice).await.unwrap();
        assert_eq!(refreshed.display_name, "alice");
        assert_eq!(refreshed.email.as_deref(), Some("alice@example.org"));
    }

    #[tokio::test]
    async fn claim_administration_is_applied_and_manual_kept() {
        let database = crate::testing::database().await;
        let root = create_local(&database, local("root", None), &POLICY)
            .await
            .unwrap();
        let mut alice = identity("alice", "alice");
        let mut bob = identity("bob", "bob");

        alice.administrator = Some(true);
        let granted = sign_in_with_identity(&database, &alice).await.unwrap();
        assert!(granted.is_administrator);
        assert!(granted.administrator_from_claim);

        alice.administrator = None;
        let unchanged = sign_in_with_identity(&database, &alice).await.unwrap();
        assert!(unchanged.is_administrator);

        alice.administrator = Some(false);
        let revoked = sign_in_with_identity(&database, &alice).await.unwrap();
        assert!(!revoked.is_administrator);
        assert!(!revoked.administrator_from_claim);

        let created = sign_in_with_identity(&database, &bob).await.unwrap();
        assert!(!created.is_administrator);
        set_administrator(&database, root.id, created.id, true)
            .await
            .unwrap();
        bob.administrator = Some(false);
        let kept = sign_in_with_identity(&database, &bob).await.unwrap();
        assert!(kept.is_administrator);
        assert!(!kept.administrator_from_claim);

        // A claim that agrees with a manual grant does not take it over.
        bob.administrator = Some(true);
        let confirmed = sign_in_with_identity(&database, &bob).await.unwrap();
        assert!(confirmed.is_administrator);
        assert!(!confirmed.administrator_from_claim);
        bob.administrator = Some(false);
        let still = sign_in_with_identity(&database, &bob).await.unwrap();
        assert!(still.is_administrator);
        assert!(!still.administrator_from_claim);
    }

    #[tokio::test]
    async fn groups_are_replaced_when_known_and_kept_when_unknown() {
        let database = crate::testing::database().await;
        let mut alice = identity("s", "alice");

        alice.groups = Some(vec!["a".to_owned(), "b".to_owned()]);
        let user = sign_in_with_identity(&database, &alice).await.unwrap();
        assert_eq!(group_names(&database, user.id).await, ["a", "b"]);

        alice.groups = Some(vec!["b".to_owned()]);
        sign_in_with_identity(&database, &alice).await.unwrap();
        assert_eq!(group_names(&database, user.id).await, ["b"]);

        alice.groups = None;
        sign_in_with_identity(&database, &alice).await.unwrap();
        assert_eq!(group_names(&database, user.id).await, ["b"]);

        // A known claim with no groups is not an unknown one: it clears the memberships.
        alice.groups = Some(Vec::new());
        sign_in_with_identity(&database, &alice).await.unwrap();
        assert!(group_names(&database, user.id).await.is_empty());

        // Groups are never deleted automatically, even once empty.
        assert_eq!(groups::Entity::find().count(&database).await.unwrap(), 2);
        assert_eq!(
            group_memberships::Entity::find()
                .count(&database)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn claimed_groups_join_existing_ones_and_create_the_others() {
        let database = crate::testing::database().await;
        let mut bob = identity("b", "bob");
        bob.groups = Some(vec!["lab".to_owned()]);
        let bob = sign_in_with_identity(&database, &bob).await.unwrap();
        let mut alice = identity("s", "alice");
        alice.groups = Some(vec!["staff".to_owned(), "x".to_owned()]);
        let user = sign_in_with_identity(&database, &alice).await.unwrap();

        alice.groups = Some(vec![
            "staff".to_owned(),
            "lab".to_owned(),
            "new".to_owned(),
            "new".to_owned(),
            String::new(),
        ]);
        sign_in_with_identity(&database, &alice).await.unwrap();
        assert_eq!(
            group_names(&database, user.id).await,
            ["lab", "new", "staff"]
        );
        assert_eq!(group_names(&database, bob.id).await, ["lab"]);
        assert_eq!(groups::Entity::find().count(&database).await.unwrap(), 4);

        // The same groups again change nothing.
        sign_in_with_identity(&database, &alice).await.unwrap();
        assert_eq!(
            group_names(&database, user.id).await,
            ["lab", "new", "staff"]
        );
    }

    #[tokio::test]
    async fn disabled_user_cannot_sign_in() {
        let database = crate::testing::database().await;
        let root = create_local(&database, local("root", None), &POLICY)
            .await
            .unwrap();
        let alice = identity("s", "alice");
        let user = sign_in_with_identity(&database, &alice).await.unwrap();
        set_password(&database, user.id, PASSWORD.to_owned(), &POLICY)
            .await
            .unwrap();

        set_disabled(&database, root.id, user.id, true)
            .await
            .unwrap();

        assert!(matches!(
            sign_in_with_identity(&database, &alice).await,
            Err(Error::Disabled)
        ));
        assert!(matches!(
            sign_in_with_password(&database, "alice", PASSWORD.to_owned()).await,
            Err(Error::InvalidCredentials)
        ));

        set_disabled(&database, root.id, user.id, false)
            .await
            .unwrap();
        assert!(sign_in_with_identity(&database, &alice).await.is_ok());
    }

    #[tokio::test]
    async fn password_sign_in_accepts_username_or_email() {
        let database = crate::testing::database().await;
        let created = create_local(
            &database,
            local("alice", Some("Alice@Example.org")),
            &POLICY,
        )
        .await
        .unwrap();
        assert_eq!(created.email.as_deref(), Some("alice@example.org"));
        assert!(created.last_sign_in_at.is_none());

        let by_username = sign_in_with_password(&database, "alice", PASSWORD.to_owned())
            .await
            .unwrap();
        let by_email = sign_in_with_password(&database, "ALICE@example.org", PASSWORD.to_owned())
            .await
            .unwrap();

        assert_eq!(by_username.id, created.id);
        assert_eq!(by_email.id, created.id);
        assert!(by_email.last_sign_in_at.is_some());
    }

    #[tokio::test]
    async fn password_failures_look_the_same() {
        let database = crate::testing::database().await;
        let root = create_local(&database, local("root", None), &POLICY)
            .await
            .unwrap();
        create_local(&database, local("alice", None), &POLICY)
            .await
            .unwrap();
        sign_in_with_identity(&database, &identity("s", "bob"))
            .await
            .unwrap();
        let carol = create_local(&database, local("carol", None), &POLICY)
            .await
            .unwrap();
        set_disabled(&database, root.id, carol.id, true)
            .await
            .unwrap();

        for (login, password) in [
            ("nobody", PASSWORD),
            ("alice", "not the right password"),
            ("bob", PASSWORD),
            ("carol", PASSWORD),
        ] {
            assert!(
                matches!(
                    sign_in_with_password(&database, login, password.to_owned()).await,
                    Err(Error::InvalidCredentials)
                ),
                "{login} should get InvalidCredentials"
            );
        }
    }

    #[tokio::test]
    async fn an_overlong_password_is_refused_unhashed() {
        let database = crate::testing::database().await;
        let alice = create_local(&database, local("alice", None), &POLICY)
            .await
            .unwrap();
        let longest = "a".repeat(passwords::MAXIMUM_LENGTH);
        set_password(&database, alice.id, longest.clone(), &POLICY)
            .await
            .unwrap();
        assert!(
            sign_in_with_password(&database, "alice", longest)
                .await
                .is_ok()
        );

        // Even the right password is refused past the bound, since it is never compared. The
        // policy never accepts such a password, so it is stored directly.
        let overlong = "a".repeat(passwords::MAXIMUM_LENGTH + 1);
        let mut changes: users::ActiveModel = reload(&database, alice.id).await.into();
        changes.password_hash = Set(Some(passwords::hash(overlong.clone()).await));
        changes.update(&database).await.unwrap();
        assert!(matches!(
            sign_in_with_password(&database, "alice", overlong).await,
            Err(Error::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn local_accounts_are_validated() {
        let database = crate::testing::database().await;
        create_local(
            &database,
            local("alice", Some("alice@example.org")),
            &POLICY,
        )
        .await
        .unwrap();

        let refused = [
            (local("Alice", None), Problem::UsernameInvalid),
            (local("alice", None), Problem::UsernameTaken),
            (local("bob", Some("ALICE@example.org")), Problem::EmailTaken),
            (
                NewLocalUser {
                    password: "short".to_owned(),
                    ..local("bob", None)
                },
                Problem::PasswordTooShort { minimum: 12 },
            ),
            (
                NewLocalUser {
                    password: "a".repeat(1025),
                    ..local("bob", None)
                },
                Problem::PasswordTooLong { maximum: 1024 },
            ),
        ];
        for (user, problem) in refused {
            match create_local(&database, user, &POLICY).await {
                Err(Error::Invalid(refused)) => assert_eq!(refused, problem),
                other => panic!("expected {problem:?}, got {other:?}"),
            }
        }
        assert_eq!(user_count(&database).await, 1);
    }

    /// What a creation that passed the checks but lost the race to the unique index gets.
    #[tokio::test]
    async fn a_lost_race_on_the_unique_index_is_a_taken_name() {
        let database = crate::testing::database().await;
        create_local(
            &database,
            local("alice", Some("alice@example.org")),
            &POLICY,
        )
        .await
        .unwrap();
        let insert = |username: &str, email: &str| users::ActiveModel {
            username: Set(username.to_owned()),
            display_name: Set(username.to_owned()),
            email: Set(Some(email.to_owned())),
            is_administrator: Set(false),
            created_at: Set(Utc::now().fixed_offset()),
            ..Default::default()
        };
        for (model, problem) in [
            (insert("alice", "other@example.org"), Problem::UsernameTaken),
            (insert("bob", "alice@example.org"), Problem::EmailTaken),
        ] {
            let error = model.insert(&database).await.unwrap_err();
            match taken(error) {
                Error::Invalid(refused) => assert_eq!(refused, problem),
                other => panic!("expected {problem:?}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn setting_a_password_ends_sessions() {
        let database = crate::testing::database().await;
        let user = create_local(&database, local("alice", None), &POLICY)
            .await
            .unwrap();
        assert!(user.sessions_valid_after.is_none());

        let before = chrono::Utc::now();
        set_password(
            &database,
            user.id,
            "another long password".to_owned(),
            &POLICY,
        )
        .await
        .unwrap();
        let first = reload(&database, user.id)
            .await
            .sessions_valid_after
            .unwrap();
        assert!(first > before);

        set_password(
            &database,
            user.id,
            "yet another password".to_owned(),
            &POLICY,
        )
        .await
        .unwrap();
        let second = reload(&database, user.id)
            .await
            .sessions_valid_after
            .unwrap();
        assert!(second > first);

        assert!(
            sign_in_with_password(&database, "alice", "yet another password".to_owned())
                .await
                .is_ok()
        );
        assert!(matches!(
            set_password(&database, user.id, "short".to_owned(), &POLICY).await,
            Err(Error::Invalid(Problem::PasswordTooShort { .. }))
        ));
        assert!(matches!(
            set_password(&database, user.id + 1, PASSWORD.to_owned(), &POLICY).await,
            Err(Error::NotFound)
        ));
    }

    #[tokio::test]
    async fn disabling_ends_sessions_and_promoting_is_manual() {
        let database = crate::testing::database().await;
        let root = create_local(&database, local("root", None), &POLICY)
            .await
            .unwrap();
        // Administration granted by a claim cannot be changed by hand (see
        // `administration_from_the_claim_is_not_changed_by_hand`), so this user has none.
        let user = sign_in_with_identity(&database, &identity("s", "alice"))
            .await
            .unwrap();

        let before = chrono::Utc::now();
        set_disabled(&database, root.id, user.id, true)
            .await
            .unwrap();
        assert!(
            reload(&database, user.id)
                .await
                .sessions_valid_after
                .unwrap()
                > before
        );

        set_administrator(&database, root.id, user.id, true)
            .await
            .unwrap();
        let promoted = reload(&database, user.id).await;
        assert!(promoted.is_administrator);
        assert!(!promoted.administrator_from_claim);
    }

    #[tokio::test]
    async fn nobody_disables_or_demotes_themselves() {
        let database = crate::testing::database().await;
        let root = create_local(
            &database,
            NewLocalUser {
                is_administrator: true,
                ..local("root", None)
            },
            &POLICY,
        )
        .await
        .unwrap();

        assert!(matches!(
            set_disabled(&database, root.id, root.id, true).await,
            Err(Error::Invalid(Problem::OwnAccount))
        ));
        assert!(matches!(
            set_administrator(&database, root.id, root.id, false).await,
            Err(Error::Invalid(Problem::OwnAccount))
        ));
        let unchanged = reload(&database, root.id).await;
        assert!(unchanged.is_administrator);
        assert!(!unchanged.is_disabled);
    }

    #[tokio::test]
    async fn administration_from_the_claim_is_not_changed_by_hand() {
        let database = crate::testing::database().await;
        let root = create_local(&database, local("root", None), &POLICY)
            .await
            .unwrap();
        let mut alice = identity("alice", "alice");
        alice.administrator = Some(true);
        let alice = sign_in_with_identity(&database, &alice).await.unwrap();

        for administrator in [false, true] {
            assert!(matches!(
                set_administrator(&database, root.id, alice.id, administrator).await,
                Err(Error::Invalid(Problem::AdministratorFromClaim))
            ));
            let unchanged = reload(&database, alice.id).await;
            assert!(unchanged.is_administrator);
            assert!(unchanged.administrator_from_claim);
        }

        // The command line still can, for when the claim is no longer configured.
        set_administrator_from_command_line(&database, alice.id, false)
            .await
            .unwrap();
        let removed = reload(&database, alice.id).await;
        assert!(!removed.is_administrator);
        assert!(!removed.administrator_from_claim);
    }

    #[tokio::test]
    async fn any_with_password_needs_a_password() {
        let database = crate::testing::database().await;
        assert!(!any_with_password(&database).await.unwrap());

        let alice = sign_in_with_identity(&database, &identity("s", "alice"))
            .await
            .unwrap();
        assert!(!any_with_password(&database).await.unwrap());

        set_password(&database, alice.id, PASSWORD.to_owned(), &POLICY)
            .await
            .unwrap();
        assert!(any_with_password(&database).await.unwrap());
    }

    #[tokio::test]
    async fn users_are_listed_with_their_groups_and_sign_in_methods() {
        let database = crate::testing::database().await;
        create_local(&database, local("carol", None), &POLICY)
            .await
            .unwrap();
        let mut bob = identity("s", "bob");
        bob.groups = Some(vec!["family".to_owned(), "admins".to_owned()]);
        sign_in_with_identity(&database, &bob).await.unwrap();

        let summaries = list(&database).await.unwrap();

        let usernames: Vec<_> = summaries
            .iter()
            .map(|summary| summary.user.username.as_str())
            .collect();
        assert_eq!(usernames, ["bob", "carol"]);
        assert_eq!(summaries[0].groups, ["admins", "family"]);
        assert!(summaries[0].has_identity && !summaries[0].has_password);
        assert!(summaries[1].groups.is_empty());
        assert!(!summaries[1].has_identity && summaries[1].has_password);
        assert_eq!(
            find_by_username(&database, "bob")
                .await
                .unwrap()
                .map(|user| user.id),
            Some(summaries[0].user.id)
        );
        assert!(find_by_username(&database, "Bob").await.unwrap().is_none());
    }
}
