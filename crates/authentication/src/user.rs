use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The signed-in person, kept in the session and shown in the interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub subject: String,
    pub username: String,
    pub name: Option<String>,
}

impl User {
    /// Reads the user from ID token or UserInfo claims. The username is `preferred_username`,
    /// else `email`, else `sub`, so there is always something to show.
    pub(crate) fn from_claims(claims: &Map<String, Value>) -> Option<Self> {
        let text = |key: &str| {
            claims
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let subject = text("sub")?;
        let username = text("preferred_username")
            .or_else(|| text("email"))
            .unwrap_or_else(|| subject.clone());
        Some(Self {
            subject,
            username,
            name: text("name"),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn user(value: Value) -> User {
        User::from_claims(value.as_object().unwrap()).unwrap()
    }

    #[test]
    fn username_falls_back_to_email_then_subject() {
        let full = user(json!({"sub": "s", "preferred_username": "a", "email": "a@x.org"}));
        assert_eq!(full.username, "a");
        assert_eq!(
            user(json!({"sub": "s", "email": "a@x.org"})).username,
            "a@x.org"
        );
        assert_eq!(user(json!({"sub": "s"})).username, "s");
        assert!(User::from_claims(&Map::new()).is_none());
    }
}
