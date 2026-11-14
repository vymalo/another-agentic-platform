//! What the review of K9 asked for: the whole document is checked, a null is absent, a key twice is refused, the settings are
//! read as the operator reads them, a directory skips what is not ours, the line has one shape, and the behaviours that a
//! change to the code could have broken without a test noticing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::*;
use serde_json::{Value, json};

/// An example with changes, written to `dir` under `file`.
fn changed(dir: &Path, file: &str, name: &str, change: impl Fn(&mut Value)) -> PathBuf {
    let mut docs = read_docs(&example(name));
    docs.iter_mut().for_each(change);
    let path = dir.join(file);
    write_docs(&path, &docs);
    path
}

fn of_kind<'a>(kind: &'a str, then: impl Fn(&mut Value) + 'a) -> impl Fn(&mut Value) + 'a {
    move |d| {
        if d["kind"] == kind {
            then(d);
        }
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

fn validate_with_image(file: &Path) -> Out {
    aapctl(
        &["validate", "-f", s(file), "--default-agent-image", IMAGE],
        None,
    )
}

#[test]
fn unknown_fields_are_found_in_the_whole_document() {
    let dir = scratch("unknown-whole");
    let cases: [(&str, &str, &str, &str, Value); 4] = [
        (
            "chat.yaml",
            "AgentService",
            "/metadata/nmae",
            "metadata.nmae",
            json!("x"),
        ),
        ("chat.yaml", "AgentService", "/bogus", "bogus", json!(1)),
        (
            "chat.yaml",
            "AgentConfig",
            "/metadata/lables",
            "metadata.lables",
            json!({"a": "b"}),
        ),
        (
            "coder-me.yaml",
            "AgentService",
            "/spec/access/allowFrom/0/namespaceSelectr",
            "spec.access.allowFrom[0].namespaceSelectr",
            json!({"a": 1}),
        ),
    ];
    for (example_name, kind, pointer, path, value) in cases {
        let file = changed(
            &dir,
            "x.yaml",
            example_name,
            of_kind(kind, |d| set(d, pointer, value.clone())),
        );
        let out = validate_with_image(&file);
        assert_eq!(out.code, 1, "{path}: {}", out.stderr);
        assert!(
            out.stderr.contains(&format!(": {path}: unknown field")),
            "{path}: {}",
            out.stderr
        );
        assert!(
            out.stderr.contains(&format!("x.yaml: {kind}/")),
            "{}",
            out.stderr
        );
    }
    // `status` is the operator's: ignored in an input, whatever it holds
    let file = changed(
        &dir,
        "status.yaml",
        "chat.yaml",
        of_kind("AgentService", |d| {
            set(d, "/status", json!({"anything": [1, 2]}));
        }),
    );
    let out = validate_with_image(&file);
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn a_key_given_twice_is_refused() {
    let doc = "apiVersion: agents.vymalo.com/v1alpha1\nkind: AgentService\nmetadata:\n  name: x\nspec:\n  description: a\n  description: b\n";
    let out = aapctl(&["validate", "-f", "-"], Some(doc));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("<stdin>"), "{}", out.stderr);
    assert!(out.stderr.contains("description"), "{}", out.stderr);
}

#[test]
fn a_null_is_an_absent_field_as_an_api_server_reads_it() {
    let dir = scratch("nulls");
    for (kind, pointer) in [
        ("AgentService", "/spec/interfaces/a2a/publicUrl"),
        ("AgentService", "/spec/access"),
        ("AgentService", "/spec/scaling/workers"),
        ("AgentService", "/spec/description"),
        ("AgentConfig", "/spec/tools"),
    ] {
        let file = changed(
            &dir,
            "n.yaml",
            "chat.yaml",
            of_kind(kind, |d| set(d, pointer, Value::Null)),
        );
        let out = validate_with_image(&file);
        assert_eq!(out.code, 0, "{pointer}: {}", out.stderr);
    }
    // an empty map or list is a value too, and the type may leave it out when it writes the object back
    let file = changed(&dir, "empty.yaml", "chat.yaml", |d| {
        if d["kind"] == "AgentService" {
            set(d, "/spec/access/allowFrom", json!([]));
        } else {
            set(d, "/spec/tools/mcpServers", json!({}));
        }
    });
    let out = validate_with_image(&file);
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn settings_that_the_operator_would_not_start_with_are_usage_errors() {
    let chat = s(&example("chat.yaml")).to_owned();
    for args in [
        vec!["render", "-f", &chat, "--owner-uid", ""],
        vec!["render", "-f", &chat, "--owner-uid", "  "],
        vec!["validate", "-f", &chat, "--default-agent-image", "a b"],
        vec!["render", "-f", &chat, "--default-agent-image", "a b"],
    ] {
        let out = aapctl(&args, None);
        assert_eq!(out.code, 2, "{args:?}: {}", out.stderr);
        assert_eq!(out.stdout, "");
    }
    // a blank image is none, as the operator reads a blank DEFAULT_AGENT_IMAGE
    assert_eq!(
        aapctl(
            &["validate", "-f", &chat, "--default-agent-image", " "],
            None
        )
        .code,
        0
    );
}

#[test]
fn the_run_pods_file_is_read_as_the_operator_reads_it() {
    let dir = scratch("run-pods-file");
    let chat = s(&example("chat.yaml")).to_owned();
    // blank: none
    assert_eq!(
        aapctl(&["validate", "-f", &chat, "--run-pods-file", " "], None).code,
        0
    );
    // missing
    let missing = dir.join("missing.yaml");
    assert_eq!(
        aapctl(
            &["validate", "-f", &chat, "--run-pods-file", s(&missing)],
            None
        )
        .code,
        2
    );
    // parses, and fails its own checks
    let good = chart_run_pod_settings(&dir);
    let mut settings: Value =
        serde_yaml::from_str(&std::fs::read_to_string(&good).unwrap()).unwrap();
    set(&mut settings, "/quota/pods", json!(0));
    let bad = dir.join("bad.yaml");
    std::fs::write(&bad, serde_yaml::to_string(&settings).unwrap()).unwrap();
    let out = aapctl(&["validate", "-f", &chat, "--run-pods-file", s(&bad)], None);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("quota.pods"), "{}", out.stderr);
    // does not parse (an unknown key)
    set(&mut settings, "/quota/pods", json!(4));
    set(&mut settings, "/bogus", json!(1));
    std::fs::write(&bad, serde_yaml::to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        aapctl(&["validate", "-f", &chat, "--run-pods-file", s(&bad)], None).code,
        2
    );
}

#[test]
fn a_directory_skips_what_is_not_ours_and_a_named_file_does_not() {
    let dir = scratch("directory-foreign");
    std::fs::copy(example("chat.yaml"), dir.join("chat.yaml")).unwrap();
    std::fs::write(
        dir.join("deploy.yaml"),
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: d\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("kustomization.yaml"),
        "apiVersion: kustomize.config.k8s.io/v1beta1\nkind: Kustomization\nresources: [chat.yaml]\n",
    )
    .unwrap();
    std::fs::write(dir.join("plain.yaml"), "resources: [a]\n").unwrap();
    std::fs::write(dir.join("list.yaml"), "- a\n- b\n").unwrap();
    let out = validate_with_image(&dir);
    assert_eq!(out.code, 0, "{}", out.stderr);
    for skipped in [
        "deploy.yaml",
        "kustomization.yaml",
        "plain.yaml",
        "list.yaml",
    ] {
        let notes: Vec<_> = out
            .stderr
            .lines()
            .filter(|l| {
                l.contains(skipped) && l.contains("skipped: not an agents.vymalo.com object")
            })
            .collect();
        assert_eq!(notes.len(), 1, "{skipped}: {}", out.stderr);
    }
    assert_eq!(lines(&out.stderr).len(), 4, "{}", out.stderr);

    // our group, and not what we read: an error even in a directory
    for (file, text, word) in [
        (
            "v1.yaml",
            "apiVersion: agents.vymalo.com/v1\nkind: AgentService\nmetadata:\n  name: z\n",
            "agents.vymalo.com/v1",
        ),
        (
            "kind.yaml",
            "apiVersion: agents.vymalo.com/v1alpha1\nkind: Bogus\nmetadata:\n  name: z\n",
            "Bogus",
        ),
    ] {
        let inside = scratch("directory-ours");
        std::fs::write(inside.join(file), text).unwrap();
        let out = aapctl(&["validate", "-f", s(&inside)], None);
        assert_eq!(out.code, 2, "{file}: {}", out.stderr);
        assert!(out.stderr.contains(word), "{file}: {}", out.stderr);
        // and as a named file, too
        let out = aapctl(&["validate", "-f", s(&inside.join(file))], None);
        assert_eq!(out.code, 2, "{file}: {}", out.stderr);
    }

    // named as a file, or on stdin, a foreign document is an error that says what it was
    for (text, what) in [
        (
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: d\n",
            "Deployment of apps/v1",
        ),
        ("- a\n- b\n", "a list"),
        ("hello\n", "a scalar"),
        (
            "kind: AgentService\nmetadata:\n  name: z\n",
            "no apiVersion",
        ),
    ] {
        let file = dir.join("named.txt");
        std::fs::write(&file, text).unwrap();
        let out = aapctl(&["validate", "-f", s(&file)], None);
        assert_eq!(out.code, 2, "{text}: {}", out.stderr);
        assert!(out.stderr.contains(what), "{what}: {}", out.stderr);
        let out = aapctl(&["validate", "-f", "-"], Some(text));
        assert_eq!(out.code, 2, "{text}: {}", out.stderr);
        assert!(
            out.stderr.contains("<stdin>") && out.stderr.contains(what),
            "{}",
            out.stderr
        );
    }
}

#[test]
fn an_issue_line_is_file_object_field_message() {
    let me = s(&example("coder-me.yaml")).to_owned();
    let out = aapctl(&["validate", "-f", &me], None);
    assert_eq!(out.code, 1);
    let line = out.stderr.lines().next().unwrap();
    assert!(
        line.starts_with(&format!(
            "{me}: AgentConfig/coder-me: spec.environment.image: no image"
        )),
        "{line}"
    );
    // the flag of this command where the operator's message names its variable
    assert!(line.ends_with("(aapctl: --default-agent-image)"), "{line}");
    assert!(!line.contains("AgentConfig spec"), "{line}");

    // a shared object: the object is the line's, and not repeated in the field
    let dir = scratch("line-shared");
    let paired = invalid_with_partner(
        &root().join("examples/invalid/model-endpoint-base-url-both.yaml"),
        &dir,
    );
    let out = aapctl(&["validate", "-f", s(&paired)], None);
    assert_eq!(out.code, 1);
    assert!(
        out.stderr
            .lines()
            .any(|l| l.contains(": ModelEndpoint/invalid: spec.")),
        "{}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("ModelEndpoint invalid"),
        "{}",
        out.stderr
    );

    // a document a type cannot read: no path, no `spec:`
    let text = std::fs::read_to_string(example("chat.yaml"))
        .unwrap()
        .replace("topology: combined", "topology: sideways");
    let out = aapctl(&["validate", "-f", "-"], Some(&text));
    let line = out.stderr.lines().next().unwrap();
    assert!(
        line.starts_with("<stdin>: AgentService/chat: cannot be read: "),
        "{line}"
    );
    assert!(!line.contains("spec:"), "{line}");

    // with more than one namespace the namespace is in the object
    let dir = scratch("line-namespaces");
    let docs: Vec<Value> = read_docs(&example("chat.yaml"))
        .into_iter()
        .map(|mut d| {
            set(&mut d, "/metadata/namespace", json!("other"));
            if d["kind"] == "AgentService" {
                set(&mut d, "/spec/bogus", json!(1));
            }
            d
        })
        .chain(read_docs(&example("coder.yaml")))
        .collect();
    let file = dir.join("two.yaml");
    write_docs(&file, &docs);
    let out = aapctl(&["validate", "-f", s(&file)], None);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr.contains(": AgentService/other/chat: spec.bogus"),
        "{}",
        out.stderr
    );
}

#[test]
fn paths_are_read_once_in_any_case_and_odd_entries_are_noted() {
    let chat = s(&example("chat.yaml")).to_owned();
    assert_eq!(
        aapctl(&["validate", "-f", &chat, "-f", &chat], None).code,
        0
    );
    let dir = scratch("paths");
    std::fs::copy(example("chat.yaml"), dir.join("A.YAML")).unwrap();
    std::fs::copy(example("coder.yaml"), dir.join("b.YML")).unwrap();
    let a = dir.join("A.YAML");
    assert_eq!(
        aapctl(
            &["validate", "-f", s(&dir), "-f", s(&a), "-f", s(&dir)],
            None
        )
        .code,
        0,
        "a file reached by two paths is read once"
    );
    // the extensions are matched in any case: an invalid document in `b.YML` is found
    let bad = changed(
        &dir,
        "b.YML",
        "coder.yaml",
        of_kind("AgentService", |d| set(d, "/spec/bogus", json!(1))),
    );
    let out = aapctl(&["validate", "-f", s(&dir)], None);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains(s(&bad)), "{}", out.stderr);
    #[cfg(unix)]
    {
        std::fs::remove_file(&bad).unwrap();
        std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("broken.yaml")).unwrap();
        std::fs::create_dir(dir.join("sub.yaml")).unwrap();
        let out = aapctl(&["validate", "-f", s(&dir)], None);
        assert_eq!(out.code, 0, "{}", out.stderr);
        for odd in ["broken.yaml", "sub.yaml"] {
            assert!(
                out.stderr
                    .lines()
                    .any(|l| l.contains(odd) && l.contains("skipped: not a regular file")),
                "{odd}: {}",
                out.stderr
            );
        }
    }
}

#[test]
fn the_same_name_in_two_namespaces_is_two_objects_and_a_lookup_stays_in_its_own() {
    let dir = scratch("namespaces");
    let docs: Vec<Value> = ["ns-a", "ns-b"]
        .iter()
        .flat_map(|ns| {
            let ns = *ns;
            read_docs(&example("coder.yaml"))
                .into_iter()
                .map(move |mut d| {
                    set(&mut d, "/metadata/namespace", json!(ns));
                    d
                })
        })
        .collect();
    let both = dir.join("both.yaml");
    write_docs(&both, &docs);
    assert_eq!(aapctl(&["validate", "-f", s(&both)], None).code, 0);
    let out = aapctl(&["render", "-f", s(&both)], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("# AgentService ns-a/coder: "),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("# AgentService ns-b/coder: "),
        "{}",
        out.stdout
    );

    // a config in another namespace is not the service's config
    let docs: Vec<Value> = read_docs(&example("coder.yaml"))
        .into_iter()
        .map(|mut d| {
            if d["kind"] == "AgentConfig" {
                set(&mut d, "/metadata/namespace", json!("ns-b"));
            }
            d
        })
        .collect();
    let apart = dir.join("apart.yaml");
    write_docs(&apart, &docs);
    let out = aapctl(&["validate", "-f", s(&apart)], None);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr.contains("does not exist in namespace"),
        "{}",
        out.stderr
    );

    // nor is a model endpoint of another namespace the config's
    let moved = changed(
        &dir,
        "moved.yaml",
        "coder-me.yaml",
        of_kind("ModelEndpoint", |d| {
            set(d, "/metadata/namespace", json!("elsewhere"));
        }),
    );
    let out = validate_with_image(&moved);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr.contains("ModelEndpoint") && out.stderr.contains("was not found"),
        "{}",
        out.stderr
    );
}

#[test]
fn every_file_of_a_directory_is_judged_in_name_order() {
    let dir = scratch("directory-order");
    std::fs::copy(example("chat.yaml"), dir.join("a.yaml")).unwrap();
    let b = changed(
        &dir,
        "b.yml",
        "coder.yaml",
        of_kind("AgentService", |d| set(d, "/spec/bogus", json!(1))),
    );
    let out = aapctl(&["validate", "-f", s(&dir)], None);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains(s(&b)), "{}", out.stderr);

    let a = changed(
        &dir,
        "a.yaml",
        "chat.yaml",
        of_kind("AgentService", |d| set(d, "/spec/bogus", json!(1))),
    );
    let out = aapctl(&["validate", "-f", s(&dir)], None);
    let at = |path: &Path| {
        out.stderr
            .find(s(path))
            .unwrap_or_else(|| panic!("{}", out.stderr))
    };
    assert!(at(&a) < at(&b), "{}", out.stderr);
}

#[test]
fn the_cache_and_the_access_tier_are_the_deployments() {
    let dir = scratch("deployment");
    let cached = changed(
        &dir,
        "cache.yaml",
        "coder.yaml",
        of_kind("AgentConfig", |d| {
            set(d, "/spec/environment/cache", json!({"existingClaim": "c"}));
        }),
    );
    assert_eq!(aapctl(&["validate", "-f", s(&cached)], None).code, 1);
    let out = aapctl(
        &["validate", "-f", s(&cached), "--cache-allowed-claims", "c"],
        None,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let out = aapctl(
        &[
            "validate",
            "-f",
            s(&cached),
            "--cache-allowed-claims",
            "x,c",
        ],
        None,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);

    let tier = changed(
        &dir,
        "tier.yaml",
        "coder.yaml",
        of_kind("AgentConfig", |d| {
            set(d, "/spec/harness/adam/coder/maxAccessTier", json!("bypass"));
        }),
    );
    // the operator's default ceiling is `auto`
    assert_eq!(aapctl(&["validate", "-f", s(&tier)], None).code, 1);
    assert_eq!(
        aapctl(
            &["validate", "-f", s(&tier), "--max-access-tier", "auto"],
            None
        )
        .code,
        1
    );
}

/// The digest annotation of the objects of one rendered service.
fn digests_in(segment: &str) -> Vec<String> {
    documents(segment)
        .iter()
        .filter_map(|d| {
            d["metadata"]["annotations"]["agents.vymalo.com/config-digest"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn render_of_several_services_keeps_each_digest_and_each_object() {
    let (coder, chat) = (
        s(&example("coder.yaml")).to_owned(),
        s(&example("chat.yaml")).to_owned(),
    );
    let both = aapctl(&["render", "-f", &coder, "-f", &chat], None);
    assert_eq!(both.code, 0, "{}", both.stderr);
    let alone = |f: &str| {
        let out = aapctl(&["render", "-f", f], None);
        assert_eq!(out.code, 0);
        out.stdout
    };
    // one separator between the two, and the objects of each as they are alone
    assert_eq!(
        both.stdout.matches("\n---\n# AgentService ").count(),
        1,
        "{}",
        both.stdout
    );
    let total = |text: &str| documents(text).len();
    assert_eq!(
        total(&both.stdout),
        total(&alone(&coder)) + total(&alone(&chat))
    );

    for segment in both.stdout.split("\n---\n# AgentService ") {
        let segment = segment.strip_prefix("# AgentService ").unwrap_or(segment);
        let (head, rest) = segment.split_once('\n').unwrap();
        let name = head
            .split(':')
            .next()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_owned();
        let digest = head.split(": ").nth(1).unwrap().to_owned();
        let found = digests_in(rest);
        assert!(!found.is_empty(), "{name}: no object carries a digest");
        assert!(
            found.iter().all(|d| *d == digest),
            "{name}: {digest} vs {found:?}"
        );
    }
    // and the two services have two digests
    let heads: Vec<&str> = both
        .stdout
        .lines()
        .filter(|l| l.starts_with("# AgentService "))
        .collect();
    assert_eq!(heads.len(), 2);
    assert_ne!(heads[0].split(": ").nth(1), heads[1].split(": ").nth(1));
}

#[test]
fn render_prints_nothing_when_anything_is_invalid_and_does_not_forgive_a_missing_referent() {
    let (coder, me) = (
        s(&example("coder.yaml")).to_owned(),
        s(&example("coder-me.yaml")).to_owned(),
    );
    // coder-me names no image and there is no default: one invalid object spoils the whole stream
    let out = aapctl(&["render", "-f", &coder, "-f", &me], None);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert_eq!(out.stdout, "");

    let dir = scratch("render-missing");
    let mut docs = read_docs(&example("coder-me.yaml"));
    docs.retain(|d| d["kind"] != "ModelEndpoint");
    let no_endpoint = dir.join("me.yaml");
    write_docs(&no_endpoint, &docs);
    let args = [
        "render",
        "-f",
        s(&no_endpoint),
        "--default-agent-image",
        IMAGE,
    ];
    let out = aapctl(&args, None);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert_eq!(out.stdout, "");
    assert!(out.stderr.contains("ModelEndpoint"), "{}", out.stderr);
    // and there is no flag to forgive it
    let mut forgiving = args.to_vec();
    forgiving.push("--allow-missing-refs");
    assert_eq!(aapctl(&forgiving, None).code, 2);
}

#[test]
fn what_no_service_reaches_is_still_judged() {
    for name in [
        "model-ref-and-api-key.yaml",
        "tool-provider-url-empty.yaml",
        "model-endpoint-base-url-both.yaml",
    ] {
        let file = root().join("examples/invalid").join(name);
        let out = aapctl(&["validate", "-f", s(&file)], None);
        assert_eq!(out.code, 1, "{name}: {}", out.stderr);
        assert!(out.stderr.contains(name), "{name}: {}", out.stderr);
    }
    // an invalid endpoint next to a service that uses it: the line is the endpoint's
    let paired = invalid_with_partner(
        &root().join("examples/invalid/model-endpoint-base-url-both.yaml"),
        &scratch("attribution"),
    );
    let out = aapctl(&["validate", "-f", s(&paired)], None);
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains(": ModelEndpoint/invalid: "),
        "{}",
        out.stderr
    );
    assert!(
        !out.stderr.contains(": AgentService/coder: spec.model"),
        "{}",
        out.stderr
    );

    // a typo in a config is one line, and not again for the service that names it
    let dir = scratch("typo");
    let typo = changed(
        &dir,
        "chat.yaml",
        "chat.yaml",
        of_kind("AgentConfig", |d| {
            let harness = d["spec"]
                .as_object_mut()
                .unwrap()
                .remove("harness")
                .unwrap();
            set(d, "/spec/harnes", harness);
        }),
    );
    let out = validate_with_image(&typo);
    assert_eq!(out.code, 1);
    assert_eq!(lines(&out.stderr).len(), 1, "{}", out.stderr);
}

#[test]
fn no_secret_value_is_printed_by_either_command() {
    let dir = scratch("secrets");
    for (pointer, value, secret) in [
        (
            "/spec/model/apiKeySecretRef",
            json!("sk-SECRETVALUE-123"),
            "SECRETVALUE",
        ),
        (
            "/spec/model/baseUrl",
            json!("https://user:PASSWORDVALUE@gateway.example.com/v1"),
            "PASSWORDVALUE",
        ),
    ] {
        let file = changed(
            &dir,
            "s.yaml",
            "chat.yaml",
            of_kind("AgentConfig", |d| set(d, pointer, value.clone())),
        );
        for command in ["validate", "render"] {
            let out = aapctl(
                &[command, "-f", s(&file), "--default-agent-image", IMAGE],
                None,
            );
            assert!(
                !out.stdout.contains(secret) && !out.stderr.contains(secret),
                "{command} {pointer}: {}{}",
                out.stdout,
                out.stderr
            );
        }
    }
}

#[test]
fn a_bom_crlf_an_empty_file_and_stdin_twice() {
    let text = std::fs::read_to_string(example("chat.yaml")).unwrap();
    let bom = format!("\u{feff}{text}");
    assert_eq!(aapctl(&["validate", "-f", "-"], Some(&bom)).code, 0);
    let crlf = text.replace('\n', "\r\n");
    assert_eq!(aapctl(&["validate", "-f", "-"], Some(&crlf)).code, 0);
    let out = aapctl(&["validate", "-f", "-"], Some("---\n"));
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("nothing to read"), "{}", out.stderr);
    let out = aapctl(&["validate", "-f", "-", "-f", "-"], Some(&text));
    assert_eq!(out.code, 0, "{}", out.stderr);
}
