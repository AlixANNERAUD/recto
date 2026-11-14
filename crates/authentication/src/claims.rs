//! Checking the claim an operator requires to let someone in.
//!
//! A claim is looked up as a whole name first (`urn:zitadel:iam:org:project:roles` is one
//! claim), then as a dot-separated path into nested objects (`realm_access.roles` for Keycloak).

use serde_json::{Map, Value};

fn lookup<'a>(claims: &'a Map<String, Value>, claim: &str) -> Option<&'a Value> {
    if let Some(value) = claims.get(claim) {
        return Some(value);
    }
    let mut segments = claim.split('.');
    let mut value = claims.get(segments.next()?)?;
    for segment in segments {
        value = value.as_object()?.get(segment)?;
    }
    Some(value)
}

/// Whether the claims satisfy the requirement. Without a `value`, the claim only has to be
/// present (not `null`, not `false`). With one, a string claim must equal it, an array must
/// contain it, and an object must have it as a key (Zitadel lists roles as object keys).
pub fn allows(claims: &Map<String, Value>, claim: &str, value: Option<&str>) -> bool {
    let Some(found) = lookup(claims, claim) else {
        return false;
    };
    let Some(expected) = value else {
        return !matches!(found, Value::Null | Value::Bool(false));
    };
    match found {
        Value::String(text) => text == expected,
        Value::Array(items) => items.iter().any(|item| item.as_str() == Some(expected)),
        Value::Object(map) => map.contains_key(expected),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn claims(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn string_claim_equal() {
        let claims = claims(json!({"role": "printer"}));
        assert!(allows(&claims, "role", Some("printer")));
        assert!(!allows(&claims, "role", Some("admin")));
    }

    #[test]
    fn array_claim_contains() {
        let claims = claims(json!({"groups": ["staff", "printers"]}));
        assert!(allows(&claims, "groups", Some("printers")));
        assert!(!allows(&claims, "groups", Some("print")));
    }

    #[test]
    fn zitadel_role_object_has_key() {
        let claims = claims(json!({
            "urn:zitadel:iam:org:project:roles": {"recto": {"123": "example.com"}}
        }));
        assert!(allows(
            &claims,
            "urn:zitadel:iam:org:project:roles",
            Some("recto")
        ));
        assert!(!allows(
            &claims,
            "urn:zitadel:iam:org:project:roles",
            Some("admin")
        ));
    }

    #[test]
    fn dotted_path() {
        let claims = claims(json!({"realm_access": {"roles": ["offline_access", "recto"]}}));
        assert!(allows(&claims, "realm_access.roles", Some("recto")));
        assert!(!allows(&claims, "realm_access.missing", Some("recto")));
    }

    #[test]
    fn missing_claim_denies() {
        let claims = claims(json!({"other": "x"}));
        assert!(!allows(&claims, "role", Some("x")));
        assert!(!allows(&claims, "role", None));
    }

    #[test]
    fn presence_only() {
        let claims = claims(json!({"yes": true, "text": "x", "no": false, "null": null}));
        assert!(allows(&claims, "yes", None));
        assert!(allows(&claims, "text", None));
        assert!(!allows(&claims, "no", None));
        assert!(!allows(&claims, "null", None));
    }
}
