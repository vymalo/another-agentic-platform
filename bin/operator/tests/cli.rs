//! The binary's contract: `crdgen` prints exactly the checked-in CRDs, `run` refuses until S5.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::path::Path;
use std::process::Command;

fn operator() -> Command {
    Command::new(env!("CARGO_BIN_EXE_operator"))
}

#[test]
fn crdgen_matches_the_checked_in_crds() {
    let out = operator()
        .arg("crdgen")
        .output()
        .expect("runs the operator");
    assert!(
        out.status.success(),
        "crdgen failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let generated = String::from_utf8(out.stdout).expect("utf-8");

    let golden =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/crds/agents.vymalo.com.yaml");
    let checked_in = std::fs::read_to_string(&golden).unwrap_or_default();
    assert!(
        generated == checked_in,
        "{} differs from the output of `crdgen`. Regenerate it:\n  cargo run -q -p aap-operator -- crdgen > deploy/crds/agents.vymalo.com.yaml",
        golden.display()
    );
}

#[test]
fn crdgen_prints_one_document_per_kind() {
    let out = operator()
        .arg("crdgen")
        .output()
        .expect("runs the operator");
    let text = String::from_utf8(out.stdout).expect("utf-8");
    assert_eq!(
        text.matches("\n---\n").count(),
        2,
        "one `---` before each of the two CRDs"
    );
    assert!(text.contains("name: agentconfigs.agents.vymalo.com"));
    assert!(text.contains("name: agentservices.agents.vymalo.com"));
}

#[test]
fn run_says_it_is_not_implemented_until_s5() {
    let out = operator().arg("run").output().expect("runs the operator");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not implemented until slice S5"),
        "{stderr}"
    );
}
