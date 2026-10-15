//! The syntax rules of the screens of §60a ("Screens", "Create a coder"), as pure string functions.
//! A rule the operator also holds (`aap-domain::validate`) is not repeated here: this is what the
//! dashboard says before it asks the operator's rules.

/// An agent's name: `^[a-z][a-z0-9-]{0,38}[a-z0-9]$`. At most 40 characters so that every name derived
/// from it (`<name>-front`, `<name>-agent-<hash>`, `<name>-db-app`, the use permission) fits.
pub(crate) fn is_agent_name(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=40).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[b.len() - 1] != b'-'
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A model endpoint's name: `^[a-z][a-z0-9-]{0,62}$`.
pub(crate) fn is_model_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=63).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A tool server's name: `^[a-z][a-z0-9-]{0,30}$`; it is the prefix of its tools, `<name>__<tool>`.
pub(crate) fn is_tool_server_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=31).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A tag: 1 to 64 characters, lower-case words joined by single dashes.
pub(crate) fn is_tag(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// A GitHub login: `^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$`. `*` is not one.
pub(crate) fn is_github_login(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=39).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

/// Visible ASCII, `!` to `~`: the characters of an audience value (decided by the owner on 2026-10-15).
pub(crate) fn is_visible_ascii(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| (b'!'..=b'~').contains(&c))
}

/// Characters, as a person counts them (a byte count would punish an accent).
pub(crate) fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Whether `s` has a name in it: not empty, and not only white space.
pub(crate) fn has_text(s: &str) -> bool {
    !s.trim().is_empty()
}

/// The longest prefix of `s` of at most `n` characters, for a message that quotes a value.
pub(crate) fn shown(s: &str, n: usize) -> String {
    let cut: String = s.chars().take(n).collect();
    if chars(s) > n {
        format!("{cut}…")
    } else {
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names() {
        for ok in ["ab", "coder-me", "a1", "chat", &"a".repeat(40)] {
            assert!(is_agent_name(ok), "{ok}");
        }
        for bad in [
            "",
            "a",
            "A",
            "1a",
            "-a",
            "a-",
            "a_b",
            "coder.me",
            &"a".repeat(41),
            "é",
        ] {
            assert!(!is_agent_name(bad), "{bad}");
        }
    }

    #[test]
    fn tags() {
        for ok in ["coding", "git", "a-b-c", "web3", &"a".repeat(64)] {
            assert!(is_tag(ok), "{ok}");
        }
        for bad in ["", "A", "a--b", "-a", "a-", "a b", "a_b", &"a".repeat(65)] {
            assert!(!is_tag(bad), "{bad}");
        }
    }

    #[test]
    fn github_logins() {
        for ok in ["vymalo", "stephane-segning", "a", "A1", &"a".repeat(39)] {
            assert!(is_github_login(ok), "{ok}");
        }
        for bad in ["", "*", "-a", "a b", "a,b", "a_b", &"a".repeat(40)] {
            assert!(!is_github_login(bad), "{bad}");
        }
    }

    #[test]
    fn tool_server_and_model_names() {
        assert!(is_tool_server_name("websearch") && is_tool_server_name(&"a".repeat(31)));
        assert!(!is_tool_server_name(&"a".repeat(32)) && !is_tool_server_name("Web"));
        assert!(is_model_name("gateway") && is_model_name(&"a".repeat(63)));
        assert!(!is_model_name(&"a".repeat(64)) && !is_model_name("1gateway"));
    }

    #[test]
    fn visible_ascii_has_no_trailing_newline_loophole() {
        assert!(is_visible_ascii("agent.use:chat") && is_visible_ascii("*"));
        for bad in ["", "a b", "a\n", "a\t", "é", "\u{7f}"] {
            assert!(!is_visible_ascii(bad), "{bad:?}");
        }
    }
}
