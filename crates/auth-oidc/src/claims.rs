//! What a validated token says about the person: the subject and the permissions.
//!
//! The signature, the issuer, the audience and the times are checked before this runs; this reads the claims
//! the deployment configured (`rolesClaim`). Nothing here puts a value of a claim in an error.

use aap_ports::{InvalidToken, Principal};
use serde_json::{Map, Value};

/// A claim by its name, then, when `path` has dots and there is no claim of that exact name (a namespaced
/// claim such as `https://example.com/roles` has dots in its name), by the path through the objects:
/// `realm_access.roles`.
pub(crate) fn lookup<'a>(claims: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    if let Some(value) = claims.get(path) {
        return Some(value);
    }
    let (head, rest) = path.split_once('.')?;
    let mut value = claims.get(head)?;
    for part in rest.split('.') {
        value = value.as_object()?.get(part)?;
    }
    Some(value)
}

/// The permissions at `path`: the texts of a list (anything else in it is skipped, and so is an empty text),
/// or a single text. A claim that is absent or of another kind is no permission at all, and **never refuses
/// the token**: a token without roles is a person who may do nothing, not a stranger.
pub(crate) fn roles(claims: &Map<String, Value>, path: &str) -> Vec<String> {
    match lookup(claims, path) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .filter(|role| !role.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::String(role)) if !role.is_empty() => vec![role.clone()],
        _ => Vec::new(),
    }
}

/// The principal a token with `claims` names: its `sub`, which must be text, and the values at the roles claim.
/// The subject is opaque and kept byte for byte, but it must be somebody: not blank, and without a control
/// character (a newline or a NUL would have to be escaped everywhere the subject is written).
///
/// # Errors
///
/// [`InvalidToken::MissingClaim`] when `sub` is absent, is not text, is empty or only white space, or has a
/// control character.
pub(crate) fn principal(
    claims: &Map<String, Value>,
    roles_claim: &str,
) -> Result<Principal, InvalidToken> {
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.trim().is_empty() && !sub.chars().any(char::is_control))
        .ok_or(InvalidToken::MissingClaim)?;
    Ok(Principal::new(subject, roles(claims, roles_claim)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claims(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_claim_is_found_by_its_exact_name_before_its_path() {
        let c = claims(json!({
            "https://example.com/roles": ["x"],
            "realm_access": {"roles": ["r"], "deep": {"er": ["d"]}},
            "a.b": "flat",
            "a": {"b": "nested"},
        }));
        assert_eq!(lookup(&c, "https://example.com/roles"), Some(&json!(["x"])));
        assert_eq!(lookup(&c, "realm_access.roles"), Some(&json!(["r"])));
        assert_eq!(lookup(&c, "realm_access.deep.er"), Some(&json!(["d"])));
        assert_eq!(lookup(&c, "a.b"), Some(&json!("flat")));
        assert_eq!(lookup(&c, "realm_access.nope"), None);
        assert_eq!(lookup(&c, "nope.roles"), None);
        assert_eq!(lookup(&c, "realm_access.roles.x"), None);
    }

    #[test]
    fn roles_are_the_texts_and_nothing_makes_them_fail() {
        let at = |v: Value| roles(&claims(json!({"g": v})), "g");
        assert_eq!(
            at(json!(["b", "a", "b", "", 3, null, {"x": 1}])),
            ["b", "a", "b"],
            "the texts, in order: the set is the principal's"
        );
        assert_eq!(at(json!("solo")), ["solo"]);
        assert!(at(json!("")).is_empty());
        assert!(at(json!(7)).is_empty());
        assert!(at(json!(true)).is_empty());
        assert!(at(json!({"a": 1})).is_empty());
        assert!(roles(&claims(json!({})), "g").is_empty());
    }

    #[test]
    fn the_principal_is_the_subject_and_the_whole_roles_claim() {
        let c = claims(json!({"sub": "s-1", "agentic_roles": ["platform:agents.read", "user"]}));
        let p = principal(&c, "agentic_roles").unwrap();
        assert_eq!(p.subject(), "s-1");
        assert!(p.holds("platform:agents.read") && p.holds("user"));
        assert_eq!(p.permissions().count(), 2);
    }

    #[test]
    fn a_subject_that_is_not_usable_text_is_a_missing_claim() {
        for value in [
            json!({}),
            json!({"sub": ""}),
            json!({"sub": 5}),
            json!({"sub": null}),
            json!({"sub": ["a"]}),
            json!({"sub": {"a": 1}}),
        ] {
            assert_eq!(
                principal(&claims(value), "r").unwrap_err(),
                InvalidToken::MissingClaim
            );
        }
        // Not trimmed, not lower-cased: the subject is opaque.
        let p = principal(&claims(json!({"sub": " Mixed Case "})), "r").unwrap();
        assert_eq!(p.subject(), " Mixed Case ");
    }

    #[test]
    fn a_subject_that_is_blank_or_has_a_control_character_is_a_missing_claim() {
        for sub in ["  ", "\t", " \n ", "a\nb\0", "a\u{7f}", "\u{1b}[0m", "x\ry"] {
            assert_eq!(
                principal(&claims(json!({ "sub": sub })), "r").unwrap_err(),
                InvalidToken::MissingClaim,
                "{sub:?}"
            );
        }
        for sub in ["Zoë", "a b", " padded ", "idp|42"] {
            let p = principal(&claims(json!({ "sub": sub })), "r").unwrap();
            assert_eq!(p.subject(), sub);
        }
    }
}
