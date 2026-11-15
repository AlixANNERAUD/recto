//! Rules for the usernames administrators type when they create a local account.
//!
//! Single sign-on usernames do not follow them: they are kept as the provider sends them (see
//! [`crate::users::sign_in_with_identity`]).

/// Maximum length of a local username, in bytes (they are ASCII).
pub const MAXIMUM_LENGTH: usize = 64;

/// Whether a local username is valid: 1 to 64 characters among lower-case ASCII letters, digits,
/// `.`, `_` and `-`, starting with a letter or a digit (nanobom's rules). Unlike nanobom, upper
/// case is refused rather than folded: the username is the job owner CUPS records, so the name
/// the administrator typed must be the one stored.
pub fn is_valid_local_username(value: &str) -> bool {
    let valid_characters = value.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || "._-".contains(character)
    });
    let valid_start = value
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit());
    value.len() <= MAXIMUM_LENGTH && valid_characters && valid_start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_usernames_follow_the_rules() {
        for value in ["alice", "a.b_c-1", "7", &"a".repeat(64)] {
            assert!(is_valid_local_username(value), "{value:?} should be valid");
        }
        for value in [
            "",
            "Alice",
            "a b",
            "-leading",
            ".hidden",
            "slash/name",
            "é",
            &"a".repeat(65),
        ] {
            assert!(
                !is_valid_local_username(value),
                "{value:?} should be invalid"
            );
        }
    }
}
