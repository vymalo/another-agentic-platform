//! The YAML of the export, written by hand because the library's would be read wrongly.
//!
//! `serde_yaml_ng` writes the strings `yes`, `no`, `on`, `off`, `y`, `n`, `12:30`, `1_000` and `2026-10-16`
//! **unquoted**. The reader a person uses to apply an export (`kubectl`, Argo CD, Flux: YAML 1.1 in effect, through
//! go-yaml) reads them as a boolean, an integer and a timestamp, so a value that was a string in the object
//! is another type in the file that was exported from it, and the object a GitOps tool applies is not the
//! one the dashboard showed.
//!
//! This writer takes a JSON value and writes block-style YAML in the layout the library used (a sequence under a
//! key is not indented). **A string is written plain only if it is made of a short list of safe characters and no
//! YAML 1.1 type resolves it to anything but a string** (`plain`); otherwise it is double-quoted with
//! JSON-like escapes, or, if it has lines and nothing in them that a literal block cannot hold, a literal
//! block. A map key follows the same rule as a value. The rule is conservative: it quotes more than it needs
//! (every string that starts like a number, a version `1.2.3` included), and never less.

use serde_json::Value;

/// The document, ending with a newline.
pub(crate) fn to_string(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Object(m) if !m.is_empty() => block(value, 0, &mut out),
        Value::Array(a) if !a.is_empty() => block(value, 0, &mut out),
        // A document that is a scalar (the export never writes one): a string never as a block, which has no
        // parent to be indented under.
        Value::String(s) => {
            out.push_str(&text(s));
            out.push('\n');
        }
        other => {
            scalar_line(other, 0, &mut out);
            out.remove(0);
        }
    }
    out
}

fn pad(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

/// The words YAML 1.1 reads as a boolean or null, in any case (`y`, `n`, `yes`, `no`, `true`, `false`, `on`,
/// `off`, `null`, `~`: YAML 1.1's list, and the other cases a reader may fold).
fn is_keyword(s: &str) -> bool {
    const WORDS: &[&str] = &[
        "y", "n", "yes", "no", "true", "false", "on", "off", "null", "~",
    ];
    WORDS.iter().any(|w| s.eq_ignore_ascii_case(w))
}

/// Whether `s` starts like a number, a sexagesimal number or a timestamp, however it goes on: a sign or a
/// digit, then only the characters such a value is made of. Over-approximate on purpose.
fn looks_like_a_number(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let starts = body
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '.');
    starts
        && body.chars().all(|c| {
            c.is_ascii_hexdigit()
                || matches!(
                    c,
                    'x' | 'X' | 'o' | 'O' | '_' | ',' | ':' | '.' | '+' | '-' | 'T' | 'Z' | ' '
                )
        })
}

/// Whether `s` may be written without quotes: safe characters only, safe at both ends, and a string for
/// every YAML 1.1 reader.
pub(crate) fn plain(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    let Some(first) = chars.peek().copied() else {
        return false;
    };
    if !(first.is_alphanumeric() || matches!(first, '_' | '/')) || s.ends_with(' ') {
        return false;
    }
    let body_ok = s.char_indices().all(|(i, c)| match c {
        c if c.is_alphanumeric() => true,
        ' ' | '_' | '.' | ',' | '/' | '(' | ')' | '-' | '+' | '@' | '[' | ']' => true,
        // `:` is part of a plain scalar unless a space or the end follows it.
        ':' => s[i + 1..].chars().next().is_some_and(|n| n != ' '),
        _ => false,
    });
    body_ok && !is_keyword(s) && !looks_like_a_number(s)
}

/// `s` in double quotes, with the escapes of JSON and the ones YAML needs besides (a line break that is
/// not `\n`, the byte order mark).
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A key or a one-line string.
fn text(s: &str) -> String {
    if plain(s) { s.to_owned() } else { quoted(s) }
}

/// Whether a literal block holds `s` exactly: it has lines, no character that a block cannot carry, and its
/// first line with text does not start with a space (the block's indentation is read from it).
fn block_safe(s: &str) -> bool {
    let body = s.trim_end_matches('\n');
    s.contains('\n')
        && !body.is_empty()
        && !body.starts_with('\n')
        && !body.chars().any(|c| {
            (c.is_control() && c != '\n' && c != '\t')
                || matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}' | '\u{feff}')
        })
        && !body.starts_with([' ', '\t'])
}

fn literal(s: &str, indent: usize, out: &mut String) {
    let body = s.trim_end_matches('\n');
    let newlines = s.len() - body.len();
    out.push_str(match newlines {
        0 => " |-\n",
        1 => " |\n",
        _ => " |+\n",
    });
    for line in body.split('\n') {
        if !line.is_empty() {
            pad(out, indent);
            out.push_str(line);
        }
        out.push('\n');
    }
    for _ in 1..newlines {
        out.push('\n');
    }
}

/// The scalar, or the empty container, of `value` on the rest of the line, with its newline; or, for a string
/// with lines, a literal block at `indent`.
fn scalar_line(value: &Value, indent: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str(" null\n"),
        Value::Bool(b) => out.push_str(if *b { " true\n" } else { " false\n" }),
        Value::Number(n) => {
            out.push(' ');
            out.push_str(&n.to_string());
            out.push('\n');
        }
        Value::String(s) if block_safe(s) => literal(s, indent, out),
        Value::String(s) => {
            out.push(' ');
            out.push_str(&text(s));
            out.push('\n');
        }
        Value::Array(_) => out.push_str(" []\n"),
        Value::Object(_) => out.push_str(" {}\n"),
    }
}

fn is_block(value: &Value) -> bool {
    match value {
        Value::Object(m) => !m.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    }
}

/// A non-empty map or sequence, each line at `indent`.
fn block(value: &Value, indent: usize, out: &mut String) {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                pad(out, indent);
                out.push_str(&text(key));
                out.push(':');
                match v {
                    // A sequence under a key is not indented: the library's layout.
                    Value::Array(_) if is_block(v) => {
                        out.push('\n');
                        block(v, indent, out);
                    }
                    Value::Object(_) if is_block(v) => {
                        out.push('\n');
                        block(v, indent + 2, out);
                    }
                    v => scalar_line(v, indent + 2, out),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                pad(out, indent);
                out.push('-');
                if is_block(item) {
                    // `- ` takes two columns: the item is a block at `indent + 2` whose first line starts
                    // after the dash.
                    let mut nested = String::new();
                    block(item, indent + 2, &mut nested);
                    out.push(' ');
                    out.push_str(&nested[indent + 2..]);
                } else {
                    scalar_line(item, indent + 2, out);
                }
            }
        }
        other => scalar_line(other, indent, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    /// What YAML 1.1 (yaml.org/type: bool, int, float, null, timestamp, merge, value) reads a plain scalar as,
    /// written from the specification's regular expressions and not from `plain`: `true` when the plain
    /// scalar is **not** a string. A reader that is a go-yaml or a PyYAML is this.
    fn yaml11_is_not_a_string(s: &str) -> bool {
        let bool_null = [
            "y", "Y", "yes", "Yes", "YES", "n", "N", "no", "No", "NO", "true", "True", "TRUE",
            "false", "False", "FALSE", "on", "On", "ON", "off", "Off", "OFF", "null", "Null",
            "NULL", "~", "",
        ];
        if bool_null.contains(&s) || s == "<<" || s == "=" {
            return true;
        }
        let digits = |t: &str, ok: &dyn Fn(char) -> bool| !t.is_empty() && t.chars().all(|c| ok(c));
        let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
        // int: binary, octal, decimal, hex, sexagesimal; float: decimal, sexagesimal, .inf, .nan
        if let Some(b) = unsigned.strip_prefix("0b")
            && digits(b, &|c| matches!(c, '0' | '1' | '_'))
        {
            return true;
        }
        if let Some(h) = unsigned.strip_prefix("0x")
            && digits(h, &|c| c.is_ascii_hexdigit() || c == '_')
        {
            return true;
        }
        if unsigned.len() > 1
            && unsigned.starts_with('0')
            && digits(&unsigned[1..], &|c| matches!(c, '0'..='7' | '_'))
        {
            return true;
        }
        if unsigned.starts_with(|c: char| c.is_ascii_digit())
            && digits(unsigned, &|c| c.is_ascii_digit() || c == '_' || c == ',')
        {
            return true;
        }
        if unsigned.contains(':') {
            let sexagesimal = unsigned
                .split(':')
                .all(|part| digits(part, &|c| c.is_ascii_digit() || c == '_' || c == '.'));
            if sexagesimal {
                return true;
            }
        }
        let lower = unsigned.to_ascii_lowercase();
        if matches!(lower.as_str(), ".inf" | ".nan") {
            return true;
        }
        // float: [-+]?([0-9][0-9_]*)?\.[0-9.]*([eE][-+][0-9]+)?
        let (mantissa, exponent) = match lower.split_once('e') {
            Some((m, e)) => (m, Some(e)),
            None => (lower.as_str(), None),
        };
        if let Some((int, frac)) = mantissa.split_once('.')
            && digits_or_empty(int, true)
            && digits_or_empty(frac, false)
            && exponent.is_none_or(|e| {
                e.strip_prefix(['+', '-'])
                    .is_some_and(|d| digits(d, &|c| c.is_ascii_digit()))
            })
        {
            return true;
        }
        // timestamp: yyyy-mm-dd, or with a time
        let date: Vec<&str> = s.split(['-', ' ', 'T', 't']).collect();
        date.len() >= 3
            && date[0].len() == 4
            && date[0].bytes().all(|c| c.is_ascii_digit())
            && date[1..3]
                .iter()
                .all(|p| p.bytes().all(|c| c.is_ascii_digit()) && !p.is_empty())
    }

    fn digits_or_empty(t: &str, underscore: bool) -> bool {
        t.chars()
            .all(|c| c.is_ascii_digit() || c == '.' || (underscore && c == '_'))
    }

    #[test]
    fn the_strings_a_yaml_1_1_reader_reads_as_something_else_are_quoted() {
        // What a YAML 1.1 reader (go-yaml, PyYAML) reads as a boolean, null, number, float or timestamp, so that
        // the check below is not a check of `plain` against itself.
        for s in [
            "yes",
            "no",
            "on",
            "off",
            "y",
            "n",
            "Y",
            "N",
            "Yes",
            "NO",
            "On",
            "OFF",
            "true",
            "False",
            "null",
            "Null",
            "~",
            "12:30",
            "1:30:45",
            "1_000",
            "1,000",
            "2026-10-16",
            "2026-10-16T12:00:00Z",
            "2026-10-16 12:00:00",
            "0x1F",
            "0b101",
            "012",
            "0",
            "-5",
            "+5",
            "1.0e+3",
            ".5",
            "-.5",
            "1.5",
            ".inf",
            "-.INF",
            ".nan",
            "<<",
            "=",
            "",
        ] {
            assert!(yaml11_is_not_a_string(s), "the check does not know {s:?}");
            assert!(!plain(s), "{s:?} would be written plain");
        }
        // What is quoted besides, for the syntax of YAML or because it starts like a number.
        for s in [
            "0o17",
            "1e3",
            " ",
            " lead",
            "trail ",
            "a: b",
            "a #b",
            "#a",
            "a:",
            ": a",
            "- a",
            "-",
            "? a",
            "[a",
            "{a",
            "&a",
            "*a",
            "!a",
            "|a",
            ">a",
            "%a",
            "@a",
            "`a",
            "a\tb",
            "multi\nline",
            "\u{85}",
            "1.2.3",
            "10001",
        ] {
            assert!(!plain(s), "{s:?} would be written plain");
        }
        for s in [
            "Coder (me)",
            "Iv1.example",
            "agent.use:coder-me",
            "https://api.github.com",
            "500m",
            "5Gi",
            "20Gi",
            "337696346+bored-giant-panda[bot]@users.noreply.github.com",
            "app.kubernetes.io/managed-by",
            "chat",
            "ü",
            "Coding task to verified pull request.",
        ] {
            assert!(plain(s), "{s:?} is quoted for no reason");
            assert!(!yaml11_is_not_a_string(s), "{s:?}");
        }
    }

    #[test]
    fn the_listed_values_are_quoted_and_the_rest_keeps_the_library_layout() {
        let value = json!({
            "answers": ["yes", "no", "on", "off", "y", "n", "Y", "N"],
            "times": ["12:30", "2026-10-16", "1_000"],
            "yes": "plain text",
            "y": { "n": "ok" },
            "empty": "",
            "map": {},
            "list": [],
            "nested": [[1, 2], { "a": 1, "b": [true, null] }],
        });
        let text = to_string(&value);
        let expected = r#"answers:
- "yes"
- "no"
- "on"
- "off"
- "y"
- "n"
- "Y"
- "N"
empty: ""
list: []
map: {}
nested:
- - 1
  - 2
- a: 1
  b:
  - true
  - null
times:
- "12:30"
- "2026-10-16"
- "1_000"
"y":
  "n": ok
"yes": plain text
"#;
        assert_eq!(text, expected);
        let back: Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(back, value, "and it reads back as the value");
    }

    #[test]
    fn a_string_with_lines_is_a_literal_block_when_a_block_holds_it_and_quoted_when_not() {
        let value = json!({
            "keep": "a\nb\n",
            "strip": "a\nb",
            "more": "a\n\n\n",
            "indented": " a\nb\n",
            "tabs": "a\n\tb\n",
            "crlf": "a\r\nb\n",
            "leading": "\na\n",
        });
        let text = to_string(&value);
        assert!(text.contains("keep: |\n  a\n  b\n"), "{text}");
        assert!(text.contains("strip: |-\n  a\n  b\n"), "{text}");
        assert!(text.contains("more: |+\n  a\n\n\n"), "{text}");
        assert!(text.contains(r#"indented: " a\nb\n""#), "{text}");
        assert!(text.contains(r#"crlf: "a\r\nb\n""#), "{text}");
        assert!(text.contains(r#"leading: "\na\n""#), "{text}");
        let back: Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(back, value);
    }

    fn arb_string() -> impl Strategy<Value = String> {
        prop_oneof![
            "[ -~]{0,12}",
            "\\PC{0,8}",
            "[a-z #:-]{0,6}(\n[a-z #:-]{0,6}){1,3}\n{0,2}",
            prop::sample::select(vec![
                "yes",
                "No",
                "on",
                "OFF",
                "y",
                "N",
                "~",
                "null",
                "12:30",
                "1_000",
                "2026-10-16",
                "0x1f",
                "1e3",
                "-.5",
                "a: b",
                "a #b",
                "- a",
                "line\nbreak\n",
                " x",
                "x ",
                "\u{2028}",
                "\u{feff}",
                "é",
                "=",
                "<<",
            ])
            .prop_map(str::to_owned),
        ]
    }

    fn arb_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            (-1000i64..1000).prop_map(|n| json!(n)),
            arb_string().prop_map(Value::String),
        ];
        leaf.prop_recursive(3, 24, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                prop::collection::btree_map(arb_string(), inner, 0..4)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

        /// Whatever the value, the document reads back as it, and a string written plain is a string to YAML 1.1.
        #[test]
        fn any_value_reads_back_as_itself(value in arb_value()) {
            let text = to_string(&value);
            let back: Value = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text:?}"));
            prop_assert_eq!(&back, &value, "{}", text);
        }

        #[test]
        fn a_plain_string_is_a_string_for_a_yaml_1_1_reader(s in arb_string()) {
            if plain(&s) {
                prop_assert!(!yaml11_is_not_a_string(&s), "{:?}", s);
                prop_assert!(!s.contains(['\n', '\r', '#', '"', '\'']), "{:?}", s);
            }
        }
    }
}
