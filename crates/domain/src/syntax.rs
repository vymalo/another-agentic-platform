//! Small syntax checks, each a rule of the adam-rs chart's `_validate.tpl` or of Kubernetes that a
//! CRD schema cannot say. Pure string functions.

/// A DNS-1123 label: lower-case letters, digits and `-`, starting and ending alphanumeric, at most
/// 63 characters.
pub fn is_dns_label(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b.first().is_some_and(|c| *c != b'-')
        && b.last().is_some_and(|c| *c != b'-')
}

/// A DNS-1123 subdomain, which is what the name of a Kubernetes object is (`IsDNS1123Subdomain`): at most
/// 253 characters of lower case letters, digits, `-` and `.`, and every `.`-separated part non-empty and
/// starting and ending with a letter or a digit. Used for the names an object has in a reference: they go
/// into the path of a request, so what is not a name must never be sent as one.
pub fn is_dns_subdomain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|part| {
            let b = part.as_bytes();
            !b.is_empty()
                && b.iter()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
                && b.first().is_some_and(|c| *c != b'-')
                && b.last().is_some_and(|c| *c != b'-')
        })
}

/// A shell-safe environment variable name: `[A-Za-z_][A-Za-z0-9_]*`. Kubernetes allows more; an
/// adam `${VAR}` reference and a person reading a manifest are served by this.
pub fn is_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An HTTP token, as a header name must be (RFC 9110 `token`): the chart's
/// `^[!#$%&'*+.^_`|~0-9A-Za-z-]+$`.
pub fn is_http_token(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+.^_`|~-".contains(c))
}

/// What `parse_http_url` found.
#[derive(Debug, PartialEq, Eq)]
pub struct HttpUrl<'a> {
    /// `http` or `https`.
    pub scheme: &'a str,
    /// Host and port, no user information.
    pub authority: &'a str,
    /// The host in lower case, without the port; an IPv6 host keeps its brackets.
    pub host: String,
}

/// Parse an `http` or `https` URL far enough to say what the operator needs: the scheme and the
/// host, and that there is no user name or password in it (a secret in a URL reaches the logs).
///
/// With `strict` the URL follows the chart's rule for an MCP server: after the authority only a
/// path or a query, and no `$` anywhere (no `${VAR}`).
pub fn parse_http_url(s: &str, strict: bool) -> Result<HttpUrl<'_>, &'static str> {
    if s.chars().any(char::is_whitespace) {
        return Err("it contains whitespace");
    }
    if strict && s.contains('$') {
        return Err("it contains `$` (no ${VAR} in a URL: it would reach the logs)");
    }
    let lower = s.to_ascii_lowercase();
    let (scheme, rest) = if lower.starts_with("https://") {
        ("https", &s["https://".len()..])
    } else if lower.starts_with("http://") {
        ("http", &s["http://".len()..])
    } else {
        return Err("it is not an http or https URL");
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() {
        return Err("it has no host");
    }
    if authority.contains('@') {
        return Err("it has a user name or password in it");
    }
    if strict && rest[end..].starts_with('#') {
        return Err("a fragment cannot follow the host");
    }
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        match stripped.find(']') {
            Some(i) => format!("[{}]", stripped[..i].to_ascii_lowercase()),
            None => return Err("its IPv6 host is not closed"),
        }
    } else {
        authority
            .split(':')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    if host.is_empty() {
        return Err("it has no host");
    }
    Ok(HttpUrl {
        scheme: &s[..scheme.len()],
        authority,
        host,
    })
}

/// A URL as an issue may show it: the scheme and the host, with the user name and password replaced by
/// `***` and everything after the host (path, query, fragment) replaced by `/…`. An issue reaches the
/// status and the Events of every service that uses the object, and a URL that is refused is the one most
/// likely to carry a secret: `https://user:password@host/`, `https://host/mcp?api_key=…`, a token in a
/// path. The field the issue names says which one, and the host says whether it is the right one.
///
/// A user name or password is recognised by an `@` before the first `?` or `#`, the last one counting,
/// so a password with a `/` in it is hidden too. A string with no scheme before its first `://` is shown
/// the same way, without one.
pub fn shown_url(s: &str) -> String {
    let (scheme, rest) = match s.find("://") {
        Some(i)
            if s[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) =>
        {
            (&s[..i + 3], &s[i + 3..])
        }
        _ => ("", s),
    };
    let head_end = rest.find(['?', '#']).unwrap_or(rest.len());
    let head = &rest[..head_end];
    let hostpath = match head.rfind('@') {
        Some(i) => format!("***@{}", &head[i + 1..]),
        None => head.to_owned(),
    };
    let (origin, path) = match hostpath.split_once('/') {
        Some((origin, _)) => (origin.to_owned(), true),
        None => (hostpath, false),
    };
    let more = path || head_end < rest.len();
    format!("{scheme}{origin}{}", if more { "/…" } else { "" })
}

/// Whether a URL is plain `http` to another machine, which adam refuses at startup unless
/// `MCP_ALLOW_INSECURE` is set: the rule of adam-rs `crates/adam-mcp/src/url.rs` as the chart
/// restates it (https, or http to `localhost`, `*.localhost`, `127.*` and `[::1]`, need nothing).
pub fn is_plain_http_remote(s: &str) -> bool {
    let Ok(url) = parse_http_url(s.trim(), false) else {
        return false;
    };
    if !url.scheme.eq_ignore_ascii_case("http") {
        return false;
    }
    let h = url.host.as_str();
    let loopback = h == "localhost"
        || h.ends_with(".localhost")
        || (h.starts_with("127.") && h.chars().all(|c| c.is_ascii_digit() || c == '.'))
        || h == "[::1]";
    !loopback
}

/// A Kubernetes quantity that is a size: [`is_quantity`] and a positive number (`500m`, `20Gi`,
/// `1e3`; not `0` or `0Gi`). Storage and volume sizes are sizes; resource requests and limits are
/// quantities, where `0` is legal.
pub fn is_size(s: &str) -> bool {
    is_quantity(s)
        && s.chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .any(|c| ('1'..='9').contains(&c))
}

/// A Kubernetes quantity: a non-negative number with an optional SI, binary or exponent suffix
/// (`0`, `500m`, `20Gi`, `1e3`).
pub fn is_quantity(s: &str) -> bool {
    let digits_end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (number, suffix) = s.split_at(digits_end);
    if number.is_empty()
        || number.matches('.').count() > 1
        || !number.chars().any(|c| c.is_ascii_digit())
    {
        return false;
    }
    const SUFFIXES: &[&str] = &[
        "", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "n", "u", "m", "k", "M", "G", "T", "P", "E",
    ];
    if SUFFIXES.contains(&suffix) {
        return true;
    }
    // An exponent: e3, E-3, e+3.
    let Some(exp) = suffix.strip_prefix(['e', 'E']) else {
        return false;
    };
    let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
    !exp.is_empty() && exp.chars().all(|c| c.is_ascii_digit())
}

/// The longest `<server>__<tool>` name adam shows a model (`MODEL_NAME_MAX`).
pub const MODEL_NAME_MAX: usize = 64;

/// Whether `name` can be the name of an MCP server in the extra MCP file, by the rule of adam-rs
/// `server_name_ok` in `crates/adam-agent-fs/src/load/mcp.rs` (*verified 2026-10-15*, read with `git show` at
/// `0391809`, the revision of the parity goldens, and the file is identical at `9a1fd4e`, the coder image the
/// chart ships; **not run**): not empty, at most 64 characters, ASCII letters, digits, `-` and `_`, no `__`
/// and not ending in `_`, because the model sees the tools as `<server>__<tool>` and `a` with a tool `_x`
/// would be `a_` with a tool `x`. The loader applies it to every server of the file, with or without a `tools`
/// list, and a file with a server that fails it is refused at start (`AgentError::ExtraMcp`), so the pod would
/// restart for ever.
pub fn is_adam_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MODEL_NAME_MAX
        && !name.contains("__")
        && !name.ends_with('_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Whether `tool` can be an entry of the `tools` allow-list of the MCP server `server`, by the rule of
/// adam-rs `crates/adam-agent-fs/src/load/mcp.rs` at `0391809` (*verified 2026-10-15*, read there): not
/// empty, not starting with `_`, ASCII letters, digits, `-` and `_` only, and `<server>__<tool>` at
/// most 64 characters. A file with an entry that fails it is refused at startup (exit 78).
pub fn is_adam_tool_name(server: &str, tool: &str) -> bool {
    !tool.is_empty()
        && !tool.starts_with('_')
        && tool
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && server.len() + 2 + tool.len() <= MODEL_NAME_MAX
}

/// A relative path inside a folder: not empty, not absolute, no empty, `.` or `..` segment, no
/// backslash and no NUL.
pub fn is_relative_file_path(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('/')
        && !s.contains(['\\', '\0'])
        && s.split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// An absolute container path with no empty, `.` or `..` segment, and not `/` itself.
pub fn is_mount_path(s: &str) -> bool {
    s.len() > 1
        && s.starts_with('/')
        && !s.contains(['\\', '\0'])
        && s[1..]
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Whether two absolute paths are equal or one is inside the other.
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let within = |outer: &str, inner: &str| {
        inner == outer
            || inner
                .strip_prefix(outer)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    within(a, b) || within(b, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_labels() {
        for ok in ["coder", "a", "a-b-1", "x".repeat(63).as_str()] {
            assert!(is_dns_label(ok), "{ok}");
        }
        for bad in ["", "-a", "a-", "A", "a_b", "a.b", "x".repeat(64).as_str()] {
            assert!(!is_dns_label(bad), "{bad}");
        }
    }

    #[test]
    fn dns_subdomains() {
        for ok in [
            "gateway",
            "a",
            "web-search",
            "a.b",
            "ns.model-endpoint.v1",
            "0",
            &"x".repeat(253),
            &format!("{}.{}", "a".repeat(63), "b".repeat(63)),
        ] {
            assert!(is_dns_subdomain(ok), "{ok}");
        }
        for bad in [
            "",
            "bad name",
            "x?watch=true",
            "x#y",
            "a/b",
            "../x",
            "..",
            ".a",
            "a.",
            "a..b",
            "-a",
            "a-",
            "a.-b",
            "UPPER",
            "a_b",
            "x%20y",
            "caf\u{e9}",
            " a",
            "a\n",
            &"x".repeat(254),
        ] {
            assert!(!is_dns_subdomain(bad), "{bad:?}");
        }
    }

    #[test]
    fn env_names() {
        for ok in ["A", "_x", "SEARCH_MCP_TOKEN", "a1"] {
            assert!(is_env_name(ok), "{ok}");
        }
        for bad in ["", "1A", "A-B", "A.B", "A B", "private-key.pem"] {
            assert!(!is_env_name(bad), "{bad}");
        }
    }

    #[test]
    fn header_names() {
        assert!(is_http_token("Authorization"));
        assert!(is_http_token("X-Api-Key"));
        assert!(!is_http_token(""));
        assert!(!is_http_token("Bad Header"));
        assert!(!is_http_token("Bad:Header"));
    }

    #[test]
    fn urls() {
        let ok = parse_http_url("https://mcp.example.com/mcp", true).unwrap();
        assert_eq!((ok.scheme, ok.host.as_str()), ("https", "mcp.example.com"));
        assert_eq!(
            parse_http_url("http://h:8080/x?y=1", true)
                .unwrap()
                .authority,
            "h:8080"
        );
        assert_eq!(
            parse_http_url("HTTP://[::1]:80/", true).unwrap().host,
            "[::1]"
        );
        for (bad, why) in [
            ("ftp://x/", "not an http"),
            ("https://u:p@x/", "user name"),
            ("https:///x", "no host"),
            ("https://x/${TOKEN}", "$"),
            ("https://x#frag", "fragment"),
            ("https://x /y", "whitespace"),
            ("", "not an http"),
        ] {
            let err = parse_http_url(bad, true).unwrap_err();
            assert!(err.contains(why), "{bad}: {err}");
        }
        // Not strict: a fragment and a `$` are the caller's business.
        assert!(parse_http_url("https://x/a$b#f", false).is_ok());
    }

    #[test]
    fn a_shown_url_has_no_user_no_password_and_no_path() {
        for (url, shown) in [
            (
                "https://u:p@gw.example.com/v1",
                "https://***@gw.example.com/…",
            ),
            ("https://bot:s3cr3t@host:8443", "https://***@host:8443"),
            ("https://x/${TOKEN}", "https://x/…"),
            ("https://host/mcp?api_key=s3cr3t", "https://host/…"),
            ("https://host?api_key=s3cr3t", "https://host/…"),
            ("https://host#s3cr3t", "https://host/…"),
            ("https://host/mcp/s3cr3t-in-the-path", "https://host/…"),
            // A password with a `/` in it, and one with an `@`: the last `@` before the query counts.
            ("https://user:pa/ss@host/mcp", "https://***@host/…"),
            ("https://user:p@ss@host/mcp", "https://***@host/…"),
            ("https://x /y", "https://x /…"),
            ("ftp://x/", "ftp://x/…"),
            ("gateway.example.invalid/v1", "gateway.example.invalid/…"),
            (
                "u:s3cr3t@gateway.example.invalid/v1",
                "***@gateway.example.invalid/…",
            ),
            ("u:s3cr3t@https://x", "***@https:/…"),
            ("https://host", "https://host"),
            ("", ""),
        ] {
            assert_eq!(shown_url(url), shown, "{url}");
            assert!(!shown_url(url).contains("s3cr3t"), "{url}");
        }
    }

    #[test]
    fn plain_http_to_another_machine() {
        for remote in [
            "http://search.ns.svc:8080/mcp",
            "http://example.com",
            "http://10.0.0.1/mcp",
            "HTTP://Example.com/x",
        ] {
            assert!(is_plain_http_remote(remote), "{remote}");
        }
        for fine in [
            "https://example.com",
            "http://localhost:3000/mcp",
            "http://api.localhost/x",
            "http://127.0.0.1:8082",
            "http://[::1]:8080/",
            "not a url",
        ] {
            assert!(!is_plain_http_remote(fine), "{fine}");
        }
    }

    #[test]
    fn adam_tool_names() {
        for ok in ["web_search", "a", "list-issues", "T1", "x_"] {
            assert!(is_adam_tool_name("search", ok), "{ok}");
        }
        // 64 characters exactly: `search` + `__` + 56.
        assert!(is_adam_tool_name("search", &"t".repeat(56)));
        for bad in ["", "_hidden", "web.search", "web search", "écho", "a/b"] {
            assert!(!is_adam_tool_name("search", bad), "{bad}");
        }
        assert!(!is_adam_tool_name("search", &"t".repeat(57)));
        // The server's name counts: the same tool does not fit behind a longer server name.
        assert!(!is_adam_tool_name(&"s".repeat(40), &"t".repeat(23)));
        assert!(is_adam_tool_name(&"s".repeat(40), &"t".repeat(22)));
    }

    #[test]
    fn adam_server_names() {
        for ok in [
            "search",
            "websearch",
            "a",
            "a_b",
            "a-b",
            "_a",
            "-a",
            "A1",
            "s01",
            "a_-_b",
            &"s".repeat(64),
        ] {
            assert!(is_adam_server_name(ok), "{ok}");
        }
        for bad in [
            "",
            "a__b",
            "__a",
            "a__",
            "a_",
            "_",
            "bad name",
            "a.b",
            "a/b",
            "caf\u{e9}",
            &"s".repeat(65),
        ] {
            assert!(!is_adam_server_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn quantities() {
        for ok in [
            "500m", "1Gi", "20Gi", "5", "0.5", "1e3", "2E-3", "100M", "1.5Gi", "0", "0m",
        ] {
            assert!(is_quantity(ok), "{ok}");
        }
        for bad in ["", "Gi", "-1", "1.2.3", "1Xi", "1e", "5 Gi", "1gi", "abc"] {
            assert!(!is_quantity(bad), "{bad}");
        }
    }

    #[test]
    fn sizes() {
        for ok in ["500m", "1Gi", "20Gi", "5", "0.5", "1e3", "1.5Gi"] {
            assert!(is_size(ok), "{ok}");
        }
        for bad in ["", "0", "0Gi", "0.0", "-1", "1Xi", "abc"] {
            assert!(!is_size(bad), "{bad}");
        }
    }

    #[test]
    fn file_paths() {
        for ok in [
            "instructions.md",
            "skills/review/SKILL.md",
            ".hidden",
            "a/b",
        ] {
            assert!(is_relative_file_path(ok), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "a/",
            "./a",
            "a\\b",
        ] {
            assert!(!is_relative_file_path(bad), "{bad}");
        }
    }

    #[test]
    fn mounts() {
        assert!(is_mount_path("/work"));
        assert!(is_mount_path("/var/lib/x"));
        for bad in ["/", "work", "/a/", "/a/../b", "//a", ""] {
            assert!(!is_mount_path(bad), "{bad}");
        }
        assert!(paths_overlap("/etc/adam", "/etc/adam/agent"));
        assert!(paths_overlap("/etc/adam/agent", "/etc/adam"));
        assert!(paths_overlap("/work", "/work"));
        assert!(!paths_overlap("/etc/adam", "/etc/adamant"));
    }
}
