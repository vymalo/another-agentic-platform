//! The image an agent may pin for itself (§60a, *Advanced*): "a reference with a tag or a digest, an
//! allow-list of registries being the deployment's default to set".
//!
//! `aap-domain` only refuses an image with a space in it, because the operator runs whatever a GitOps
//! author wrote. The dashboard is stricter, because a person types here: the reference must be a
//! reference, it must be pinned (a tag or a digest), it must not carry a user name or a password (a
//! secret in an image reference reaches the logs and the pod's spec), and, when the deployment sets an
//! allow-list, its registry must be on it.
//!
//! **What the allow-list means.** [`Defaults::allowed_image_registries`](crate::Defaults::allowed_image_registries)
//! is optional:
//!
//! * **absent**: the deployment did not restrict registries, so any registry is accepted, and the tag or
//!   digest is still required (§60a says the allow-list is "the deployment's default *to set*", so
//!   leaving it out is allowed);
//! * **a list**: the registry of the image must be one of its entries, compared as a lower-case host
//!   (and port, if the entry has one); an image with no registry in it is Docker Hub's, `docker.io`;
//! * **an empty list**: no registry is allowed, so no agent may pin its own image. It is the list
//!   somebody wrote, not the absence of one, so it fails closed.

/// The rule about an agent's own image, from the deployment's defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImagePolicy {
    /// `None`: any registry. `Some`: only these (lower-case hosts, with a port if the entry has one).
    registries: Option<Vec<String>>,
}

impl ImagePolicy {
    /// Any registry; the reference must still be pinned.
    pub fn any_registry() -> Self {
        Self::default()
    }

    /// Only these registries (`ghcr.io`, `registry.example:5000`). An empty list allows none.
    pub fn only<I, S>(registries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            registries: Some(
                registries
                    .into_iter()
                    .map(|r| r.as_ref().to_ascii_lowercase())
                    .collect(),
            ),
        }
    }

    /// Why `reference` may not be an agent's image, in words for the field `image`, or `Ok`.
    ///
    /// The message never quotes the reference: it may carry a password.
    ///
    /// # Errors
    ///
    /// The first rule the reference breaks.
    pub fn check(&self, reference: &str) -> Result<(), String> {
        let registry = registry_of(reference)?;
        match &self.registries {
            None => Ok(()),
            Some(allowed) if allowed.is_empty() => Err(
                "this deployment lets no agent pin its own image: leave the field empty for the default image"
                    .to_owned(),
            ),
            Some(allowed) if allowed.contains(&registry) => Ok(()),
            Some(allowed) => Err(format!(
                "{registry} is not an allowed registry (allowed: {})",
                allowed.join(", ")
            )),
        }
    }
}

/// The registry host of a well-formed, pinned reference without credentials, lower-cased; `docker.io`
/// when the reference names none.
fn registry_of(reference: &str) -> Result<String, String> {
    const SHAPE: &str =
        "is not an image reference: [registry/]name[:tag][@sha256:digest], with lower-case names";
    if reference.is_empty() || reference.len() > 512 {
        return Err("is 1 to 512 characters".to_owned());
    }
    if reference
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("has no space or control character in it".to_owned());
    }

    let (name, digest) = match reference.split_once('@') {
        Some((name, digest)) => (name, Some(digest)),
        None => (reference, None),
    };
    if let Some(digest) = digest
        && !is_sha256(digest)
    {
        // Everything after an `@` that is not a digest is what a URL would call the host: the part
        // before it is then a user name and a password.
        return Err(if digest.starts_with("sha256:") && !digest.contains('@') {
            "the digest is `sha256:` and 64 lower-case hexadecimal digits".to_owned()
        } else {
            "has no user name or password in it, and `@` only introduces a digest: a secret in an image reference reaches the logs"
                .to_owned()
        });
    }

    // `registry:5000/name` has a colon that is not a tag: a tag is after the last colon and has no `/`.
    let (path, tag) = match name.rsplit_once(':') {
        Some((path, tag)) if !tag.contains('/') => (path, Some(tag)),
        _ => (name, None),
    };
    if let Some(tag) = tag
        && !is_tag(tag)
    {
        return Err(
            "the tag is 1 to 128 letters, digits, `_`, `.` and `-`, not starting with `.` or `-`"
                .to_owned(),
        );
    }
    if tag.is_none() && digest.is_none() {
        return Err(
            "needs a tag or a digest (`name:tag` or `name@sha256:<64 hex digits>`): an image that can move is not pinned"
                .to_owned(),
        );
    }

    let mut parts = path.split('/');
    let first = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    // Docker's rule: the first part is a registry when there is more after it and it looks like a host.
    let (registry, repository): (String, Vec<&str>) = if !rest.is_empty()
        && (first.contains('.') || first.contains(':') || first == "localhost")
    {
        if !is_host(first) {
            return Err(SHAPE.to_owned());
        }
        (first.to_ascii_lowercase(), rest)
    } else {
        let mut all = vec![first];
        all.extend(rest);
        ("docker.io".to_owned(), all)
    };
    if repository.is_empty() || !repository.iter().all(|c| is_component(c)) {
        return Err(SHAPE.to_owned());
    }
    Ok(registry)
}

fn is_sha256(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// `[A-Za-z0-9_][A-Za-z0-9._-]{0,127}`.
fn is_tag(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=128).contains(&b.len())
        && (b[0].is_ascii_alphanumeric() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// A registry host: DNS labels, and an optional port.
pub(crate) fn is_host(s: &str) -> bool {
    let (host, port) = match s.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (s, None),
    };
    if port.is_some_and(|p| p.is_empty() || p.len() > 5 || !p.bytes().all(|c| c.is_ascii_digit())) {
        return false;
    }
    !host.is_empty()
        && host.split('.').all(|label| {
            let b = label.as_bytes();
            !b.is_empty()
                && b[0].is_ascii_alphanumeric()
                && b[b.len() - 1].is_ascii_alphanumeric()
                && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
        })
}

/// A path component of a repository: lower-case letters and digits in runs, joined by one `.`, one or
/// two `_`, or any number of `-`.
fn is_component(s: &str) -> bool {
    let b = s.as_bytes();
    let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    if b.is_empty() || !alnum(b[0]) || !alnum(b[b.len() - 1]) {
        return false;
    }
    let mut i = 0;
    while i < b.len() {
        if alnum(b[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && !alnum(b[i]) {
            i += 1;
        }
        let separator = &b[start..i];
        let ok = separator == b"."
            || separator == b"_"
            || separator == b"__"
            || separator.iter().all(|c| *c == b'-');
        if !ok {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_pinned_reference_without_credentials_is_accepted_by_any_registry() {
        let any = ImagePolicy::any_registry();
        for ok in [
            "ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e".to_owned(),
            format!("ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e@{DIGEST}"),
            format!("ghcr.io/vymalo/coder@{DIGEST}"),
            "registry.invalid:5000/a/b:1".to_owned(),
            "localhost/a:1".to_owned(),
            "ubuntu:24.04".to_owned(),
            "library/ubuntu:24.04".to_owned(),
            "REGISTRY.invalid/a:Tag_1.0-x".to_owned(),
            "a.b/c__d/e-f.g:1".to_owned(),
        ] {
            assert_eq!(any.check(&ok), Ok(()), "{ok}");
        }
    }

    #[test]
    fn a_reference_that_is_not_pinned_is_refused() {
        let any = ImagePolicy::any_registry();
        for bad in [
            "registry.invalid/a",
            "ubuntu",
            "registry.invalid:5000/a",
            "localhost/a",
        ] {
            let e = any.check(bad).unwrap_err();
            assert!(e.contains("tag or a digest"), "{bad}: {e}");
        }
    }

    #[test]
    fn credentials_and_an_at_sign_that_is_not_a_digest_are_refused_and_never_quoted() {
        let any = ImagePolicy::any_registry();
        for bad in [
            "user:pw@registry.invalid/a:1",
            "user@registry.invalid/a:1",
            "registry.invalid/a:1@registry.invalid/b:1",
            "x@y@z",
            "a:1@",
        ] {
            let e = any.check(bad).unwrap_err();
            assert!(e.contains("no user name or password"), "{bad}: {e}");
            assert!(!e.contains(bad), "the message quotes the reference: {e}");
        }
        for bad in [
            "a@sha256:abc".to_owned(),
            format!("a@sha256:{}", "A".repeat(64)),
            format!("a@sha512:{}", "a".repeat(64)),
            format!("a@{DIGEST}@{DIGEST}"),
        ] {
            assert!(any.check(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn what_is_not_a_reference_is_refused() {
        let any = ImagePolicy::any_registry();
        for bad in [
            "",
            "has space:1",
            "a\n:1",
            "a\t:1",
            "A/b:1",
            "registry.invalid/Up:1",
            "registry.invalid//a:1",
            "registry.invalid/a/:1",
            "registry.invalid/-a:1",
            "registry.invalid/a_-b:1",
            "registry.invalid/a..b:1",
            "-registry.invalid/a:1",
            "https://registry.invalid/a:1",
            "registry.invalid/a:",
            "registry.invalid/a:-1",
            "registry.invalid/a:é",
            "reg istry/a:1",
            "registry.invalid:99999999/a:1",
            "registry.invalid:/a:1",
        ] {
            assert!(any.check(bad).is_err(), "{bad:?}");
        }
        assert!(any.check(&format!("a/{}:1", "b".repeat(600))).is_err());
        assert!(any.check(&format!("a:{}", "1".repeat(129))).is_err());
    }

    #[test]
    fn the_allow_list_names_registries_and_an_empty_one_allows_none() {
        let only = ImagePolicy::only(["ghcr.io", "Registry.Invalid:5000"]);
        assert_eq!(only.check("ghcr.io/vymalo/a:1"), Ok(()));
        assert_eq!(only.check("GHCR.io/vymalo/a:1"), Ok(()));
        assert_eq!(only.check("registry.invalid:5000/a:1"), Ok(()));
        for bad in [
            "evil.example/x:1",
            "ghcr.io.evil.example/x:1",
            "registry.invalid/a:1",
            "registry.invalid:5001/a:1",
            "ubuntu:24.04",
        ] {
            let e = only.check(bad).unwrap_err();
            assert!(e.contains("is not an allowed registry"), "{bad}: {e}");
        }
        assert!(
            ImagePolicy::only(["docker.io"])
                .check("ubuntu:24.04")
                .is_ok(),
            "an image with no registry is Docker Hub's"
        );
        // The allow-list does not excuse an image that is not pinned, or has credentials.
        assert!(only.check("ghcr.io/vymalo/a").is_err());
        assert!(only.check("user:pw@ghcr.io/vymalo/a:1").is_err());

        let none = ImagePolicy::only(Vec::<String>::new());
        let e = none.check("ghcr.io/vymalo/a:1").unwrap_err();
        assert!(e.contains("no agent pin its own image"), "{e}");
    }
}
