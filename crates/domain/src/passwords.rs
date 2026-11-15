//! Local account passwords, hashed with Argon2id.
//!
//! Hashing is deliberately slow, so [`hash`] and [`verify`] run on the blocking thread pool
//! instead of stalling the asynchronous runtime. Accounts created through OpenID Connect have no
//! password unless an administrator sets one.

use std::sync::LazyLock;

use crate::error::{Error, Problem, Result};

/// Upper bound on password length, so hashing cannot be used to exhaust the server.
pub const MAXIMUM_LENGTH: usize = 1024;

/// The minimum password length when the operator sets none.
pub const DEFAULT_MINIMUM_LENGTH: usize = 12;

/// The rules a new password must follow, set by the operator. There are no composition rules:
/// length is what makes a password hard to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordPolicy {
    /// Minimum length in characters (Unicode scalar values, not bytes).
    pub minimum_length: usize,
}

impl PasswordPolicy {
    /// Checks a new password's length against the policy and against a fixed upper bound of
    /// 1024 characters.
    pub fn validate(&self, password: &str) -> Result<()> {
        let length = password.chars().count();
        if length < self.minimum_length {
            return Err(Error::Invalid(Problem::PasswordTooShort {
                minimum: self.minimum_length,
            }));
        }
        if length > MAXIMUM_LENGTH {
            return Err(Error::Invalid(Problem::PasswordTooLong {
                maximum: MAXIMUM_LENGTH,
            }));
        }
        Ok(())
    }
}

/// Hashes a password with Argon2id, off the asynchronous runtime.
pub async fn hash(password: String) -> String {
    tokio::task::spawn_blocking(move || password_auth::generate_hash(password))
        .await
        .expect("password hashing does not panic")
}

/// Checks a password against a stored hash, off the asynchronous runtime. A malformed hash, or
/// a panic of the hashing task, counts as a mismatch.
pub async fn verify(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || password_auth::verify_password(password, &hash).is_ok())
        .await
        .unwrap_or(false)
}

/// A hash of a password nobody types, with the same parameters as real hashes. Computed once,
/// on first use: hashing at each failed sign-in would double its cost.
static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| password_auth::generate_hash("recto: no account has this password"));

/// Spends the time of a [`verify`] when there is no hash to check against (unknown login, or an
/// account without a password), so the response time does not reveal which logins exist. The
/// result is meaningless and not returned.
pub(crate) async fn verify_without_hash(password: String) {
    // The hash is first computed inside the blocking task, never on the asynchronous runtime.
    let _ = tokio::task::spawn_blocking(move || {
        password_auth::verify_password(password, &DUMMY_HASH).is_ok()
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_hash_verifies_its_password_only() {
        let hashed = hash("correct horse battery staple".to_owned()).await;

        assert!(verify("correct horse battery staple".to_owned(), hashed.clone()).await);
        assert!(!verify("wrong".to_owned(), hashed).await);
        assert!(!verify("anything".to_owned(), "not a hash".to_owned()).await);
    }

    #[test]
    fn the_policy_bounds_the_length() {
        let policy = PasswordPolicy { minimum_length: 12 };

        assert!(policy.validate(&"é".repeat(12)).is_ok());
        assert!(matches!(
            policy.validate(&"a".repeat(11)),
            Err(Error::Invalid(Problem::PasswordTooShort { minimum: 12 }))
        ));
        assert!(policy.validate(&"a".repeat(1024)).is_ok());
        assert!(matches!(
            policy.validate(&"a".repeat(1025)),
            Err(Error::Invalid(Problem::PasswordTooLong { maximum: 1024 }))
        ));
    }
}
