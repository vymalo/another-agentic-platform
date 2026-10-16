//! `ETag`, `If-Match` and `If-None-Match` (RFC 9110 §8.8.3, §13.1).
//!
//! The token of an object is the port's [`ETag`]: opaque, made of the kind, name and `resourceVersion` of every object of
//! the agent (§60a). On the wire it is a **strong** entity-tag: the token in double quotes, with the bytes an entity-tag may
//! not hold written as `%XX` (the tokens of the Kubernetes adapter need none, and the escape is there so that no adapter can
//! produce a header that is not one). It is read back the same way.
//!
//! * `PUT` with `If-None-Match: *` creates; with `If-Match: "<tag>"` replaces. Neither is `428`.
//! * `DELETE` takes `If-Match: "<tag>"`. Without it is `428`.
//! * `If-Match` is compared **strongly** (RFC 9110 §13.1.1), so a weak tag (`W/"…"`) never matches: `412`.
//! * `If-Match: *` is `428`: the port has no unconditional write (two administrators must never overwrite each other
//!   silently, §60a *Revisions*), and `*` means "whatever is there".
//! * A list of tags, `If-None-Match` with anything but `*`, and both headers at once are `400`.

use aap_ports::{ETag, Precondition};
use axum::http::header::{ETAG, IF_MATCH, IF_NONE_MATCH};
use axum::http::{HeaderMap, HeaderValue};

use crate::problem::Problem;

/// What a request asks of the object it finds.
#[derive(Debug, PartialEq, Eq)]
pub enum Condition {
    /// `If-None-Match: *`.
    DoesNotExist,
    /// `If-Match: "<tag>"`.
    Matches(ETag),
}

impl Condition {
    /// The precondition a write hands to the port.
    pub fn precondition(self) -> Precondition {
        match self {
            Self::DoesNotExist => Precondition::DoesNotExist,
            Self::Matches(tag) => Precondition::Matches(tag),
        }
    }
}

/// Whether a byte stays as it is in the written token.
fn plain(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'~' | b'-')
}

/// The `ETag` header value of `tag`: quoted, strong.
pub fn header_value(tag: &ETag) -> HeaderValue {
    let mut out = String::from("\"");
    for &b in tag.as_str().as_bytes() {
        if plain(b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out.push('"');
    // Only the bytes of `plain` and `%XX` are in it.
    HeaderValue::from_str(&out).unwrap_or_else(|_| HeaderValue::from_static("\"\""))
}

/// The token a quoted value holds, or `None` when it is not one of ours.
fn unescape(inner: &str) -> Option<ETag> {
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // Two hexadecimal digits: `from_str_radix` alone would take a sign (`%+f`).
            let hex = inner.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(ETag::new)
}

/// A value of one header, or why there is none: absent is `Ok(None)`, and two of them are `400`.
fn single<'a>(
    headers: &'a HeaderMap,
    name: &axum::http::HeaderName,
) -> Result<Option<&'a str>, Problem> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(Problem::invalid_request(format!("send one {name} header")));
    }
    first
        .to_str()
        .map(|v| Some(v.trim()))
        .map_err(|_| Problem::invalid_request(format!("{name} is not text")))
}

/// What a `PUT` asks for.
///
/// # Errors
///
/// `428` when it asks for nothing (or for `If-Match: *`), `412` for a weak tag, `400` for a header that is not understood.
pub fn for_put(headers: &HeaderMap) -> Result<Condition, Problem> {
    let if_match = single(headers, &IF_MATCH)?;
    let if_none_match = single(headers, &IF_NONE_MATCH)?;
    match (if_match, if_none_match) {
        (Some(_), Some(_)) => Err(Problem::invalid_request(
            "send If-None-Match: * to create or If-Match to replace, not both",
        )),
        (None, Some("*")) => Ok(Condition::DoesNotExist),
        (None, Some(_)) => Err(Problem::invalid_request(
            "If-None-Match is understood as * only: it makes a create",
        )),
        (Some(value), None) => matching(value).map(Condition::Matches),
        (None, None) => Err(Problem::precondition_required(
            "a write is conditional: send If-None-Match: * to create, or If-Match with the ETag you read to replace",
        )),
    }
}

/// The tag a `DELETE` names.
///
/// # Errors
///
/// As [`for_put`]; a request with `If-None-Match` is `400`.
pub fn for_delete(headers: &HeaderMap) -> Result<ETag, Problem> {
    if single(headers, &IF_NONE_MATCH)?.is_some() {
        return Err(Problem::invalid_request(
            "If-None-Match makes no sense on a DELETE: send If-Match with the ETag you read",
        ));
    }
    match single(headers, &IF_MATCH)? {
        Some(value) => matching(value),
        None => Err(Problem::precondition_required(
            "a delete is conditional: send If-Match with the ETag you read",
        )),
    }
}

fn matching(value: &str) -> Result<ETag, Problem> {
    if value == "*" {
        return Err(Problem::precondition_required(
            "If-Match: * is not accepted: send the ETag of the version you read, so that nobody's work is overwritten unseen",
        ));
    }
    if value.starts_with("W/") {
        return Err(Problem::stale(
            "a weak entity-tag never matches If-Match (RFC 9110 §13.1.1): send the strong ETag as it was given",
        ));
    }
    if value.contains(',') {
        return Err(Problem::invalid_request("send one entity-tag in If-Match"));
    }
    let inner = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .filter(|v| !v.contains('"'))
        .ok_or_else(|| Problem::invalid_request("If-Match is a quoted entity-tag"))?;
    unescape(inner).ok_or_else(|| Problem::invalid_request("If-Match is not an ETag of this API"))
}

/// The `(ETAG, value)` pair to put on a response.
pub fn header(tag: &ETag) -> (axum::http::HeaderName, HeaderValue) {
    (ETAG, header_value(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(pairs: &[(&str, &str)]) -> Result<Condition, Problem> {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::try_from(*k).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        for_put(&h)
    }

    #[test]
    fn a_token_survives_the_header_whatever_it_holds() {
        for raw in ["AgentService:9:coder-me:3:123.-", "a\"b c%d", "é/ü=", ""] {
            let tag = ETag::new(raw);
            let value = header_value(&tag);
            let text = value.to_str().unwrap();
            assert!(text.starts_with('"') && text.ends_with('"'));
            assert!(!text[1..text.len() - 1].contains('"'));
            assert_eq!(matching(text).unwrap(), tag, "{raw:?}");
        }
    }

    #[test]
    fn an_escape_is_a_percent_and_two_hexadecimal_digits_and_nothing_else() {
        // `u8::from_str_radix` takes a sign: `%+f` is not an escape, and must not be read as the byte 0x0f.
        for bad in [
            "%+f", "%+0", "%-1", "%1", "%", "%1g", "%g1", "% 1", "%\u{e9}1", "abc%2",
        ] {
            assert!(unescape(bad).is_none(), "{bad:?} was read as an escape");
        }
        assert_eq!(unescape("%0f%0F%41").unwrap(), ETag::new("\u{f}\u{f}A"));
        assert_eq!(unescape("a%22b").unwrap(), ETag::new("a\"b"));
        // And through the header, the way a client sends it back.
        assert_eq!(
            matching("\"%+f\"").unwrap_err().status().as_u16(),
            400,
            "an If-Match that is not an ETag of this API"
        );
    }

    #[test]
    fn the_tokens_of_the_kubernetes_adapter_are_written_as_they_are() {
        let raw = "AgentService:9:coder-me:3:123.AgentConfig:9:coder-me:3:124";
        assert_eq!(
            header_value(&ETag::new(raw)).to_str().unwrap(),
            format!("\"{raw}\"")
        );
    }

    #[test]
    fn each_way_to_write_a_put_is_one_thing() {
        assert_eq!(
            put(&[("if-none-match", "*")]).unwrap(),
            Condition::DoesNotExist
        );
        assert_eq!(
            put(&[("if-match", "\"abc\"")]).unwrap(),
            Condition::Matches(ETag::new("abc"))
        );
        assert_eq!(put(&[]).unwrap_err().status().as_u16(), 428);
        assert_eq!(
            put(&[("if-match", "*")]).unwrap_err().status().as_u16(),
            428
        );
        assert_eq!(
            put(&[("if-match", "W/\"abc\"")])
                .unwrap_err()
                .status()
                .as_u16(),
            412
        );
        for bad in [
            &[("if-match", "abc")][..],
            &[("if-match", "\"a\", \"b\"")],
            &[("if-none-match", "\"abc\"")],
            &[("if-match", "\"a\""), ("if-none-match", "*")],
            &[("if-match", "\"a\""), ("if-match", "\"b\"")],
            &[("if-match", "\"%zz\"")],
        ] {
            assert_eq!(put(bad).unwrap_err().status().as_u16(), 400, "{bad:?}");
        }
    }
}
