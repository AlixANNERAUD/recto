/// The signed-in user of a request, read from the accounts database on every request, so that
/// disabling an account or changing its administration applies at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    /// Never changes; the CUPS `requesting-user-name` of the user's jobs.
    pub username: String,
    /// The name shown in the interface; the username when the user has none.
    pub name: String,
    pub is_administrator: bool,
    /// Whether the user has a password, so can sign in with a local account.
    pub is_local: bool,
}
