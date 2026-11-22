//! Device passwords: the credentials computers and phones print and scan with, so that a
//! person's account password never sits in a device's keychain.
//!
//! Each is a random secret of 160 bits, written as 32 lowercase base32 characters, shown once
//! when it is created and stored only as its SHA-256. A fast hash is enough for a random secret
//! of that size, and it lets every request of a device that polls every few seconds
//! authenticate with one lookup of a unique index instead of an Argon2 check. A device password
//! never signs in to the web pages: only the device routes accept it.

use chrono::{DateTime, TimeDelta, Utc};
use data_encoding::{BASE32_NOPAD, HEXLOWER};
use recto_entity::{device_passwords, users};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, TransactionSession, TransactionTrait,
};
use sha2::{Digest, Sha256};

use crate::{
    database::begin_write,
    error::{Error, Problem, Result},
};

/// How many device passwords a user may have at once.
pub const MAX_PER_USER: usize = 20;
/// The longest name of a device password, in characters.
pub const MAX_NAME_LENGTH: usize = 64;
/// The length of a secret: 20 random bytes in base32.
pub const SECRET_LENGTH: usize = 32;
const SECRET_BYTES: usize = 20;
/// How stale the last use may get before a request writes it again: devices poll every few
/// seconds, and a write per request would be most of the database's work.
pub const LAST_USE_PRECISION: TimeDelta = TimeDelta::minutes(5);

/// A device password as its owner sees it listed: never its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePassword {
    pub id: i64,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// Accurate to [`LAST_USE_PRECISION`].
    pub last_used_at: Option<DateTime<Utc>>,
}

/// A device password just created, with its secret, which is never readable again. No `Debug`,
/// so the secret cannot reach a log line by accident.
pub struct Created {
    pub id: i64,
    pub secret: String,
}

/// Who a device password authenticates, and which password it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceUser {
    pub user_id: i64,
    pub username: String,
    pub is_administrator: bool,
    pub device_password_id: i64,
    pub device_name: String,
}

/// Creates a device password for the user, named `name` once trimmed (1 to
/// [`MAX_NAME_LENGTH`] characters, no control characters), and returns its secret. Refused with
/// [`Problem::TooManyDevicePasswords`] when the user already has [`MAX_PER_USER`].
pub async fn create(
    database: &(impl ConnectionTrait + TransactionTrait),
    user_id: i64,
    name: &str,
    now: DateTime<Utc>,
) -> Result<Created> {
    let name = name.trim();
    let length = name.chars().count();
    if length == 0 || length > MAX_NAME_LENGTH || name.chars().any(char::is_control) {
        return Err(Error::Invalid(Problem::DeviceNameInvalid {
            maximum: MAX_NAME_LENGTH,
        }));
    }
    let secret = new_secret();
    // In one transaction, so two creations at once cannot both pass the count.
    let transaction = begin_write(database).await?;
    let count = device_passwords::Entity::find()
        .filter(device_passwords::Column::UserId.eq(user_id))
        .count(&transaction)
        .await?;
    if count >= MAX_PER_USER as u64 {
        return Err(Error::Invalid(Problem::TooManyDevicePasswords {
            maximum: MAX_PER_USER,
        }));
    }
    let row = device_passwords::ActiveModel {
        user_id: Set(user_id),
        name: Set(name.to_owned()),
        secret_hash: Set(hash(&secret)),
        created_at: Set(now.fixed_offset()),
        last_used_at: Set(None),
        ..Default::default()
    }
    .insert(&transaction)
    .await?;
    transaction.commit().await?;
    Ok(Created { id: row.id, secret })
}

/// The user's device passwords, newest first.
pub async fn list(database: &impl ConnectionTrait, user_id: i64) -> Result<Vec<DevicePassword>> {
    let rows = device_passwords::Entity::find()
        .filter(device_passwords::Column::UserId.eq(user_id))
        .order_by_desc(device_passwords::Column::CreatedAt)
        .order_by_desc(device_passwords::Column::Id)
        .all(database)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| DevicePassword {
            id: row.id,
            name: row.name,
            created_at: row.created_at.with_timezone(&Utc),
            last_used_at: row.last_used_at.map(|time| time.with_timezone(&Utc)),
        })
        .collect())
}

/// Deletes one of the user's device passwords; the next request of a device using it is
/// refused. `false` when the user has no such password, another user's included.
pub async fn revoke(database: &impl ConnectionTrait, user_id: i64, id: i64) -> Result<bool> {
    let result = device_passwords::Entity::delete_many()
        .filter(device_passwords::Column::Id.eq(id))
        .filter(device_passwords::Column::UserId.eq(user_id))
        .exec(database)
        .await?;
    Ok(result.rows_affected > 0)
}

/// The id of the user's device password with this secret, if they have one.
pub async fn owns(
    database: &impl ConnectionTrait,
    user_id: i64,
    secret: &str,
) -> Result<Option<i64>> {
    let Some(secret) = normalized(secret) else {
        return Ok(None);
    };
    let row = device_passwords::Entity::find()
        .filter(device_passwords::Column::SecretHash.eq(hash(&secret)))
        .filter(device_passwords::Column::UserId.eq(user_id))
        .one(database)
        .await?;
    Ok(row.map(|row| row.id))
}

/// Who `secret` authenticates: `None` when it is malformed, unknown (revoked included) or its
/// user is disabled. Read from the database on every call, so revoking or disabling applies to
/// the next request. Records the use when the last one recorded is older than
/// [`LAST_USE_PRECISION`]; failing to record it does not refuse the device.
pub async fn authenticate(
    database: &impl ConnectionTrait,
    secret: &str,
    now: DateTime<Utc>,
) -> Result<Option<DeviceUser>> {
    let Some(secret) = normalized(secret) else {
        return Ok(None);
    };
    let found = device_passwords::Entity::find()
        .filter(device_passwords::Column::SecretHash.eq(hash(&secret)))
        .find_also_related(users::Entity)
        .one(database)
        .await?;
    let Some((device, Some(user))) = found else {
        return Ok(None);
    };
    if user.is_disabled {
        return Ok(None);
    }
    let stale = device
        .last_used_at
        .is_none_or(|last| last < (now - LAST_USE_PRECISION).fixed_offset());
    if stale {
        let touched = device_passwords::ActiveModel {
            id: Set(device.id),
            last_used_at: Set(Some(now.fixed_offset())),
            ..Default::default()
        }
        .update(database)
        .await;
        if let Err(error) = touched {
            tracing::warn!(%error, device_password_id = device.id, "recording a device's last use failed");
        }
    }
    Ok(Some(DeviceUser {
        user_id: user.id,
        username: user.username,
        is_administrator: user.is_administrator,
        device_password_id: device.id,
        device_name: device.name,
    }))
}

/// A new secret: [`SECRET_BYTES`] from the system's random source, in lowercase base32.
fn new_secret() -> String {
    let mut bytes = [0; SECRET_BYTES];
    // The system's random source only fails on platforms recto does not run on.
    getrandom::fill(&mut bytes).expect("the system's random source works");
    BASE32_NOPAD.encode(&bytes).to_ascii_lowercase()
}

/// The secret in lowercase, as stored, or `None` when it cannot be one. Base32 reads the same
/// in capitals, which some password fields type.
fn normalized(secret: &str) -> Option<String> {
    let secret = secret.to_ascii_lowercase();
    let valid = secret.len() == SECRET_LENGTH
        && secret
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte));
    valid.then_some(secret)
}

/// What is stored of a secret: its SHA-256 in lowercase hex.
fn hash(secret: &str) -> String {
    HEXLOWER.encode(&Sha256::digest(secret.as_bytes()))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};
    use recto_entity::device_passwords;
    use sea_orm::{DatabaseConnection, EntityTrait};

    use super::*;
    use crate::{
        error::{Error, Problem},
        passwords::PasswordPolicy,
        users::{self, NewLocalUser},
    };

    fn noon() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 22, 12, 0, 0).unwrap()
    }

    async fn user(database: &DatabaseConnection, username: &str, administrator: bool) -> i64 {
        users::create_local(
            database,
            NewLocalUser {
                username: username.to_owned(),
                display_name: username.to_owned(),
                email: None,
                password: "correct horse battery staple".to_owned(),
                is_administrator: administrator,
            },
            &PasswordPolicy { minimum_length: 12 },
        )
        .await
        .unwrap()
        .id
    }

    async fn row(database: &DatabaseConnection, id: i64) -> device_passwords::Model {
        device_passwords::Entity::find_by_id(id)
            .one(database)
            .await
            .unwrap()
            .expect("the device password exists")
    }

    #[tokio::test]
    async fn a_secret_authenticates_its_user_until_revoked() {
        let database = crate::testing::database().await;
        let alice = user(&database, "alice", false).await;
        let bob = user(&database, "bob", false).await;
        let created = create(&database, alice, "  Laptop  ", noon())
            .await
            .unwrap();
        assert_eq!(created.secret.len(), 32);
        assert!(
            created
                .secret
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte)),
            "{}",
            created.secret
        );

        let device = authenticate(&database, &created.secret, noon())
            .await
            .unwrap()
            .expect("the secret is known");
        assert_eq!(
            device,
            DeviceUser {
                user_id: alice,
                username: "alice".into(),
                is_administrator: false,
                device_password_id: created.id,
                device_name: "Laptop".into(),
            }
        );
        // Base32 reads the same in capitals, as some password fields type it.
        let upper = created.secret.to_ascii_uppercase();
        assert!(
            authenticate(&database, &upper, noon())
                .await
                .unwrap()
                .is_some()
        );
        for wrong in [
            "",
            "short",
            &"a".repeat(32),
            &format!("{}1", &created.secret[1..]),
        ] {
            assert_eq!(authenticate(&database, wrong, noon()).await.unwrap(), None);
        }
        assert_eq!(
            owns(&database, alice, &created.secret).await.unwrap(),
            Some(created.id)
        );
        assert_eq!(owns(&database, bob, &created.secret).await.unwrap(), None);

        assert!(!revoke(&database, bob, created.id).await.unwrap());
        assert!(
            authenticate(&database, &created.secret, noon())
                .await
                .unwrap()
                .is_some()
        );
        assert!(revoke(&database, alice, created.id).await.unwrap());
        assert_eq!(
            authenticate(&database, &created.secret, noon())
                .await
                .unwrap(),
            None
        );
        assert_eq!(owns(&database, alice, &created.secret).await.unwrap(), None);
        assert!(!revoke(&database, alice, created.id).await.unwrap());
    }

    #[tokio::test]
    async fn a_disabled_user_cannot_authenticate() {
        let database = crate::testing::database().await;
        let admin = user(&database, "admin", true).await;
        let alice = user(&database, "alice", false).await;
        let created = create(&database, alice, "Phone", noon()).await.unwrap();
        users::set_disabled(&database, admin, alice, true)
            .await
            .unwrap();
        assert_eq!(
            authenticate(&database, &created.secret, noon())
                .await
                .unwrap(),
            None
        );
        users::set_disabled(&database, admin, alice, false)
            .await
            .unwrap();
        assert!(
            authenticate(&database, &created.secret, noon())
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn last_use_is_written_at_most_every_five_minutes() {
        let database = crate::testing::database().await;
        let alice = user(&database, "alice", false).await;
        let created = create(&database, alice, "Phone", noon()).await.unwrap();
        assert_eq!(row(&database, created.id).await.last_used_at, None);

        authenticate(&database, &created.secret, noon())
            .await
            .unwrap();
        let first = row(&database, created.id).await.last_used_at;
        assert_eq!(first, Some(noon().fixed_offset()));
        authenticate(&database, &created.secret, noon() + TimeDelta::minutes(1))
            .await
            .unwrap();
        assert_eq!(row(&database, created.id).await.last_used_at, first);
        let later = noon() + TimeDelta::minutes(6);
        authenticate(&database, &created.secret, later)
            .await
            .unwrap();
        assert_eq!(
            row(&database, created.id).await.last_used_at,
            Some(later.fixed_offset())
        );
        let listed = list(&database, alice).await.unwrap();
        assert_eq!(listed[0].last_used_at, Some(later));
    }

    #[tokio::test]
    async fn names_and_counts_are_limited() {
        let database = crate::testing::database().await;
        let alice = user(&database, "alice", false).await;
        let bob = user(&database, "bob", false).await;
        for name in ["", "   ", &"x".repeat(MAX_NAME_LENGTH + 1)] {
            assert!(matches!(
                create(&database, alice, name, noon()).await,
                Err(Error::Invalid(Problem::DeviceNameInvalid { .. }))
            ));
        }
        // Characters, not bytes.
        create(&database, alice, &"é".repeat(MAX_NAME_LENGTH), noon())
            .await
            .unwrap();
        for number in 1..MAX_PER_USER {
            let at = noon() + TimeDelta::seconds(number as i64);
            create(&database, alice, &format!("Device {number}"), at)
                .await
                .unwrap();
        }
        assert!(matches!(
            create(&database, alice, "One more", noon()).await,
            Err(Error::Invalid(Problem::TooManyDevicePasswords { .. }))
        ));
        // The limit is per user.
        create(&database, bob, "Phone", noon()).await.unwrap();

        let listed = list(&database, alice).await.unwrap();
        assert_eq!(listed.len(), MAX_PER_USER);
        // Newest first.
        assert_eq!(listed[0].name, format!("Device {}", MAX_PER_USER - 1));
        assert_eq!(listed[MAX_PER_USER - 1].name, "é".repeat(MAX_NAME_LENGTH));
    }

    #[tokio::test]
    async fn only_the_hash_is_stored() {
        let database = crate::testing::database().await;
        let alice = user(&database, "alice", false).await;
        let created = create(&database, alice, "Phone", noon()).await.unwrap();
        let stored = row(&database, created.id).await;
        let expected = HEXLOWER.encode(&Sha256::digest(created.secret.as_bytes()));
        assert_eq!(stored.secret_hash, expected);
        assert_eq!(stored.secret_hash.len(), 64);
        let columns = format!("{stored:?}");
        assert!(!columns.contains(&created.secret), "{columns}");
        // A second password gets another secret.
        let other = create(&database, alice, "Laptop", noon()).await.unwrap();
        assert_ne!(other.secret, created.secret);
    }
}
