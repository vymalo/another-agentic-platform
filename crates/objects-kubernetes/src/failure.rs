//! What an answer of the API server means for a write or a read.
//!
//! The mapping is held by `tests/status.rs` against `Status` bodies **captured from a kube-apiserver
//! v1.35.8** (`tests/fixtures/status`), not against bodies written from memory: three of the answers are
//! not what the API conventions suggest, and each is in a fixture.
//!
//! * A body that is not valid for the schema of the CRD (a string for a boolean, a field the CRD does not
//!   declare, a `spec` that is not an object) is refused by a **server-side apply with a `500`** and the message
//!   `failed to create typed patch object (...): .spec.suspend: expected boolean, got ...`, with no reason and no
//!   causes. It is the caller's mistake, not an outage: [`Failure::Invalid`] holds the paths that message names.
//!   (The rules of the CRD itself, the CEL rules, the bounds and `required`, are a `422` with causes.)
//! * A `resourceVersion` that is not the object's is a `409 Conflict` **with no cause**, and a field that another
//!   manager owns is a `409 Conflict` **with a `FieldManagerConflict` cause**. Both have the reason `Conflict`.
//! * A `uid` that is not the object's is a `422` ("field is immutable") for an object that exists, and a `409`
//!   for one that does not.

use aap_ports::{FieldError, Kind};
use kube::core::Status;
use kube::core::response::StatusCause;

/// What a failed call means.
#[derive(Debug)]
pub(crate) enum Failure {
    /// `404` of an object that is not there.
    NotFound(kube::Error),
    /// `404` of anything else: the API server does not serve the kind (it answers the plain text `404 page not
    /// found`, which is no `Status`), so the CRD or the External Secrets Operator is not installed.
    NotServed,
    /// `409 AlreadyExists`.
    AlreadyExists,
    /// `409`: another manager owns a field this apply would change, and `force` is off. The words of the server.
    FieldConflict(String),
    /// `409` or `422` about the `resourceVersion` or the `uid` we sent: the object is not the one that was read.
    Version,
    /// `422`, or the `500` of a body the schema refuses: the fields, in the words of the server.
    Invalid(Vec<Problem>),
    /// `5xx`, `408`, `425`, `429`, `401`, `403`, and every failure of the transport.
    Unavailable(kube::Error),
    /// Anything else.
    Internal(kube::Error),
}

/// A mistake in a body, before it is given a kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    /// From the object: `spec.access.audience`.
    pub path: String,
    pub message: String,
}

impl Problem {
    pub(crate) fn into_field(self, kind: Kind) -> FieldError {
        FieldError::new(kind, self.path, self.message)
    }
}

/// The cause of a conflict that names a field another manager owns.
const FIELD_MANAGER_CONFLICT: &str = "FieldManagerConflict";

/// The beginning of the message of a body the schema refuses (`structuredmerge.go` of the API server).
const TYPED_PATCH: &str = "failed to create typed patch object (";

/// The cause the API server adds when it has stopped checking the CEL rules because a schema error came first.
const NOT_CHECKED: &str = "some validation rules were not checked because the object was invalid";

pub(crate) fn classify(error: kube::Error) -> Failure {
    let verdict = match &error {
        kube::Error::Api(status) => verdict(status),
        kube::Error::SerdeError(_) | kube::Error::BuildRequest(_) | kube::Error::HttpError(_) => {
            Verdict::Internal
        }
        // The transport: the connection, TLS, authentication, the configuration.
        _ => Verdict::Unavailable,
    };
    match verdict {
        Verdict::Is(failure) => failure,
        Verdict::NotFound => Failure::NotFound(error),
        Verdict::Unavailable => Failure::Unavailable(error),
        Verdict::Internal => Failure::Internal(error),
    }
}

/// A [`Failure`], or one of the three that keep the error they came from.
enum Verdict {
    Is(Failure),
    NotFound,
    Unavailable,
    Internal,
}

fn verdict(status: &Status) -> Verdict {
    match status.code {
        404 if status.reason == "NotFound" => Verdict::NotFound,
        404 => Verdict::Is(Failure::NotServed),
        409 if status.reason == "AlreadyExists" => Verdict::Is(Failure::AlreadyExists),
        409 => Verdict::Is(conflict(status)),
        422 => Verdict::Is(unprocessable(status)),
        500 if status.message.starts_with(TYPED_PATCH) => {
            Verdict::Is(Failure::Invalid(typed_patch(&status.message)))
        }
        // `401` and `403` are an RBAC rule that is missing or has not propagated, as in the other providers.
        401 | 403 | 408 | 425 | 429 | 500..=599 => Verdict::Unavailable,
        _ => Verdict::Internal,
    }
}

fn causes(status: &Status) -> &[StatusCause] {
    status
        .details
        .as_ref()
        .map_or(&[][..], |d| d.causes.as_slice())
}

fn conflict(status: &Status) -> Failure {
    if causes(status)
        .iter()
        .any(|c| c.reason == FIELD_MANAGER_CONFLICT)
    {
        Failure::FieldConflict(status.message.clone())
    } else {
        Failure::Version
    }
}

fn unprocessable(status: &Status) -> Failure {
    let causes = causes(status);
    // A `uid` or a `resourceVersion` that is not the object's: nothing in the body is wrong.
    if !causes.is_empty()
        && causes
            .iter()
            .all(|c| c.field == "metadata.uid" || c.field == "metadata.resourceVersion")
    {
        return Failure::Version;
    }
    let mut problems: Vec<Problem> = causes
        .iter()
        .filter(|c| !is_noise(c))
        .map(|c| Problem {
            path: path_of(&c.field),
            message: c.message.clone(),
        })
        .collect();
    if problems.is_empty() {
        problems.push(Problem {
            path: "spec".to_owned(),
            message: status.message.clone(),
        });
    }
    Failure::Invalid(problems)
}

/// The cause that carries no field.
fn is_noise(cause: &StatusCause) -> bool {
    (cause.field == "<nil>" || cause.field.is_empty()) && cause.message.contains(NOT_CHECKED)
}

/// A field as the API server spells it (`spec.access.audience`, or `.spec.suspend` in a typed patch): from the
/// object, without a leading dot; `spec` when it names none.
fn path_of(field: &str) -> String {
    let path = field.trim_start_matches('.');
    if path.is_empty() || path == "<nil>" {
        "spec".to_owned()
    } else {
        path.to_owned()
    }
}

/// The problems of a typed patch that the schema refused. One is `...): <path>: <what>`; several are `...):
/// errors:\n  <path>: <what>\n  <path>: <what>`.
///
/// The words after `, got ` are dropped: the server prints the offending value there, in Go's notation
/// (`&{yes}`, `&value.valueUnstructured{Value:5}`), and an answer to a person must not echo a value.
fn typed_patch(message: &str) -> Vec<Problem> {
    let rest = message
        .strip_prefix(TYPED_PATCH)
        .and_then(|m| m.split_once("): "))
        .map_or(message, |(_, rest)| rest);
    let lines: Vec<&str> = match rest.strip_prefix("errors:") {
        Some(many) => many
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect(),
        None => vec![rest.trim()],
    };
    lines
        .into_iter()
        .map(|line| match line.split_once(": ") {
            Some((path, what)) => Problem {
                path: path_of(path),
                message: what.split(", got ").next().unwrap_or(what).to_owned(),
            },
            None => Problem {
                path: "spec".to_owned(),
                message: line.to_owned(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture of `tests/fixtures/status`: the body a kube-apiserver v1.35.8 answered, as the client gives it.
    fn api(file: &str, text: &str) -> kube::Error {
        let status: Status = serde_json::from_str(text).unwrap_or_else(|e| panic!("{file}: {e}"));
        kube::Error::Api(Box::new(status))
    }

    macro_rules! fixture {
        ($name:literal) => {
            api(
                $name,
                include_str!(concat!("../tests/fixtures/status/", $name)),
            )
        };
    }

    fn problems(failure: Failure) -> Vec<(String, String)> {
        match failure {
            Failure::Invalid(p) => p.into_iter().map(|p| (p.path, p.message)).collect(),
            other => panic!("not Invalid: {other:?}"),
        }
    }

    #[test]
    fn a_cel_rule_is_invalid_at_the_field_it_names() {
        let got = problems(classify(fixture!("apply-cel-audience.422.json")));
        assert_eq!(
            got,
            vec![(
                "spec.access.audience".to_owned(),
                "Invalid value: access.audience: the value * is only allowed alone".to_owned()
            )]
        );
        let got = problems(classify(fixture!("apply-cel-model-exclusive.422.json")));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "spec.model");
        assert!(got[0].1.contains("exclusive"), "{got:?}");
    }

    #[test]
    fn every_required_field_is_a_field_and_the_cause_without_one_is_dropped() {
        let got = problems(classify(fixture!("apply-required-missing.422.json")));
        assert_eq!(
            got,
            vec![("spec.configRef".to_owned(), "Required value".to_owned())]
        );
        let got = problems(classify(fixture!("apply-spec-null.422.json")));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "spec");
    }

    #[test]
    fn a_name_is_a_field_of_the_metadata() {
        let got = problems(classify(fixture!("apply-bad-name.422.json")));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "metadata.name");
    }

    #[test]
    fn a_body_the_schema_refuses_is_invalid_and_not_an_outage() {
        // The API server answers `500`, with no reason and no cause.
        let got = problems(classify(fixture!("apply-type-mismatch.500.json")));
        assert_eq!(
            got,
            vec![("spec.suspend".to_owned(), "expected boolean".to_owned())]
        );
        let got = problems(classify(fixture!("apply-string-spec.500.json")));
        assert_eq!(got, vec![("spec".to_owned(), "expected map".to_owned())]);
        let got = problems(classify(fixture!("apply-unknown-field.500.json")));
        assert_eq!(
            got,
            vec![(
                "spec.bogus".to_owned(),
                "field not declared in schema".to_owned()
            )]
        );
        let got = problems(classify(fixture!("apply-list-item-type.500.json")));
        assert_eq!(
            got,
            vec![(
                "spec.access.audience[1]".to_owned(),
                "expected string".to_owned()
            )]
        );
    }

    #[test]
    fn several_problems_of_a_typed_patch_are_all_reported() {
        let got = problems(classify(fixture!("apply-three-type-errors.500.json")));
        assert_eq!(
            got,
            vec![
                (
                    "spec.scaling.workers".to_owned(),
                    "expected numeric (int or float)".to_owned()
                ),
                ("spec.suspend".to_owned(), "expected boolean".to_owned()),
                (
                    "spec.deletionPolicy".to_owned(),
                    "expected string".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn the_value_the_server_printed_is_never_repeated() {
        for (file, text) in [
            (
                "a",
                include_str!("../tests/fixtures/status/apply-type-mismatch.500.json"),
            ),
            (
                "b",
                include_str!("../tests/fixtures/status/apply-three-type-errors.500.json"),
            ),
            (
                "c",
                include_str!("../tests/fixtures/status/apply-list-item-type.500.json"),
            ),
        ] {
            for (_, message) in problems(classify(api(file, text))) {
                assert!(!message.contains("got"), "{message}");
                assert!(!message.contains("&{"), "{message}");
                assert!(!message.contains("valueUnstructured"), "{message}");
            }
        }
    }

    #[test]
    fn an_apply_that_is_not_the_version_that_was_read_is_a_lost_race() {
        assert!(matches!(
            classify(fixture!("apply-stale-version.409.json")),
            Failure::Version
        ));
        assert!(matches!(
            classify(fixture!("apply-uid-no-object.409.json")),
            Failure::Version
        ));
        // A `uid` that is not the object's: nothing in the body is wrong.
        assert!(matches!(
            classify(fixture!("apply-uid-immutable.422.json")),
            Failure::Version
        ));
        assert!(matches!(
            classify(fixture!("delete-stale-version.409.json")),
            Failure::Version
        ));
        assert!(matches!(
            classify(fixture!("delete-uid-mismatch.409.json")),
            Failure::Version
        ));
    }

    #[test]
    fn a_field_another_manager_owns_is_a_conflict_with_the_servers_words() {
        match classify(fixture!("apply-field-manager-conflict.409.json")) {
            Failure::FieldConflict(message) => {
                assert!(message.contains("kubectl-edit"), "{message}");
                assert!(message.contains(".spec.registry.title"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_object_a_missing_kind_and_a_name_that_exists_are_told_apart() {
        assert!(matches!(
            classify(fixture!("get-not-found.404.json")),
            Failure::NotFound(_)
        ));
        assert!(matches!(
            classify(fixture!("delete-not-found.404.json")),
            Failure::NotFound(_)
        ));
        assert!(matches!(
            classify(fixture!("post-already-exists.409.json")),
            Failure::AlreadyExists
        ));
        // The plain text `404 page not found` of a kind the server does not have, as the client reconstructs it.
        let status =
            Status::failure("404 page not found\n", "Failed to parse error data").with_code(404);
        assert!(matches!(
            classify(kube::Error::Api(Box::new(status))),
            Failure::NotServed
        ));
    }

    fn status(code: u16, reason: &str) -> kube::Error {
        kube::Error::Api(Box::new(Status::failure("x", reason).with_code(code)))
    }

    #[test]
    fn an_outage_is_unavailable_and_the_rest_is_internal() {
        for code in [401, 403, 408, 425, 429, 500, 502, 503, 504] {
            assert!(
                matches!(classify(status(code, "R")), Failure::Unavailable(_)),
                "{code}"
            );
        }
        for code in [400, 405, 406, 410, 415, 418] {
            assert!(
                matches!(classify(status(code, "R")), Failure::Internal(_)),
                "{code}"
            );
        }
        // A `500` that is not a typed patch is the server's own trouble.
        let status = Status::failure("etcdserver: request timed out", "").with_code(500);
        assert!(matches!(
            classify(kube::Error::Api(Box::new(status))),
            Failure::Unavailable(_)
        ));
        // The transport.
        assert!(matches!(
            classify(kube::Error::TlsRequired),
            Failure::Unavailable(_)
        ));
        assert!(matches!(
            classify(kube::Error::Service("connection refused".into())),
            Failure::Unavailable(_)
        ));
    }

    #[test]
    fn a_conflict_without_a_cause_is_a_version_and_a_422_without_causes_is_one_problem() {
        assert!(matches!(
            classify(status(409, "Conflict")),
            Failure::Version
        ));
        let got = problems(classify(status(422, "Invalid")));
        assert_eq!(got, vec![("spec".to_owned(), "x".to_owned())]);
    }
}
