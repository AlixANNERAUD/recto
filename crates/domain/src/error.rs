//! The error type of domain operations.
//!
//! The variants are what callers branch on: the interface maps each one to a page or a status
//! without inspecting messages. Only the [`Problem`] of [`Error::Invalid`] is meant for users,
//! and the interface words it through its own locale keys; the English `Display` messages here
//! are for logs. Database errors are logged and replaced by a generic message.

use sea_orm::DbErr;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The user, or what the operation names, does not exist.
    #[error("not found")]
    NotFound,
    /// The account is disabled. Only single sign-on reports it: the password form answers
    /// [`Error::InvalidCredentials`] instead, so it does not confirm that a password is right.
    #[error("the account is disabled")]
    Disabled,
    /// The login or the password is wrong, or the account cannot sign in with a password. The
    /// cases are indistinguishable on purpose, so a failed attempt does not reveal which logins
    /// exist.
    #[error("invalid login or password")]
    InvalidCredentials,
    /// The input is invalid; the problem is safe to show to the user.
    #[error("{0}")]
    Invalid(Problem),
    /// The database failed. The message may reveal schema details, so it is logged and never
    /// shown to the user.
    #[error("database error: {0}")]
    Database(#[from] DbErr),
}

/// Why an account operation was refused, in terms the user can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    #[error(
        "the username must be 1 to 64 characters among lower-case letters, digits, `.`, `_` and \
         `-`, starting with a letter or a digit"
    )]
    UsernameInvalid,
    #[error("the username is taken")]
    UsernameTaken,
    #[error("the email address belongs to another user")]
    EmailTaken,
    #[error("the password must be at least {minimum} characters long")]
    PasswordTooShort { minimum: usize },
    #[error("the password must be at most {maximum} characters long")]
    PasswordTooLong { maximum: usize },
    /// Administrators cannot disable or demote themselves, so the last administrator cannot
    /// lock everyone out.
    #[error("you cannot change this on your own account")]
    OwnAccount,
    /// The identity provider's administrator claim granted administration, and the next sign-in
    /// would apply it again, so changing it by hand would only mislead. The command line can.
    #[error("administration of this user comes from the identity provider")]
    AdministratorFromClaim,
    /// A quota rule needs a name.
    #[error("the rule needs a name")]
    RuleNameEmpty,
    #[error("the limit cannot be negative")]
    RuleLimitNegative,
    #[error("the limit cannot be more than {maximum}")]
    RuleLimitTooLarge { maximum: i64 },
    /// Grants and resets were made in the rule's metric and for its windows: a rule that has
    /// some keeps its metric and its kind of period.
    #[error("a rule with adjustments keeps its metric and period")]
    RuleAdjusted,
    #[error("a rolling period lasts 1 to {maximum} days")]
    RollingDaysInvalid { maximum: u16 },
    /// The group or user a rule or an adjustment names does not exist.
    #[error("the group or user does not exist")]
    SubjectNotFound,
    /// A rule's list of printers has an empty name.
    #[error("a printer name is empty")]
    PrinterNameEmpty,
    #[error("the extra allowance must be more than 0")]
    GrantNotPositive,
    #[error("the extra allowance cannot be more than {maximum}")]
    GrantTooLarge { maximum: i64 },
    /// An adjustment is for the user of a personal rule, or for the group of a pool rule.
    #[error("the adjustment does not fit the rule")]
    HolderMismatch,
    #[error("the device name must be 1 to {maximum} characters")]
    DeviceNameInvalid { maximum: usize },
    #[error("the device name cannot contain control characters such as line breaks or tabs")]
    DeviceNameControl,
    #[error("a user has at most {maximum} device passwords")]
    TooManyDevicePasswords { maximum: usize },
}

/// The result of a domain operation.
pub type Result<T, E = Error> = std::result::Result<T, E>;
