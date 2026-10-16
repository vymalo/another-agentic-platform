//! The command line of the `test-issuer` example (D7b2): what it accepts and every way it refuses, each refusal naming its flag.
//! The example prints a refusal with the usage and exits 2; a test of the process itself would depend on cargo having built the example.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use aap_auth_oidc::testkit::issuer_cli::{IssuerArgs, Parsed, USAGE, UsageError, parse};
use aap_auth_oidc::testkit::{TEST_AUDIENCE, TEST_ROLES_CLAIM, TestIdp};

fn run(args: &[&str]) -> IssuerArgs {
    match parse(args) {
        Ok(Parsed::Run(args)) => args,
        other => panic!("{args:?} should run, got {other:?}"),
    }
}

/// The refusal of `args`, which must name `flag` and every fragment of `says`.
fn refused(args: &[&str], flag: &str, says: &[&str]) -> UsageError {
    let e = match parse(args) {
        Err(e) => e,
        Ok(parsed) => panic!("{args:?} should be refused, got {parsed:?}"),
    };
    assert!(
        e.0.contains(flag),
        "{args:?}: the message does not name {flag}: {e}"
    );
    for fragment in says {
        assert!(
            e.0.contains(fragment),
            "{args:?}: the message lacks '{fragment}': {e}"
        );
    }
    e
}

#[test]
fn no_argument_is_the_loopback_and_the_testkits_defaults() {
    let args = run(&[]);
    assert_eq!(args.bind, "127.0.0.1:0".parse().unwrap());
    assert_eq!(args.public_host, "127.0.0.1");
    assert_eq!(args.control, "127.0.0.1:0".parse().unwrap());
    assert_eq!(args.audience, TEST_AUDIENCE);
    assert_eq!(args.roles_claim, TEST_ROLES_CLAIM);
}

#[test]
fn what_the_end_to_end_passes_is_read_as_written() {
    let args = run(&[
        "--bind",
        "0.0.0.0:0",
        "--public-host",
        "172.18.0.1",
        "--audience",
        "aap-e2e-api",
        "--roles-claim",
        "resource_access.another-agentic.roles",
    ]);
    assert_eq!(args.bind, "0.0.0.0:0".parse().unwrap());
    assert_eq!(args.public_host, "172.18.0.1");
    assert_eq!(args.control, "127.0.0.1:0".parse().unwrap());
    assert_eq!(args.audience, "aap-e2e-api");
    assert_eq!(args.roles_claim, "resource_access.another-agentic.roles");
}

#[test]
fn help_anywhere_is_help_and_the_usage_says_the_exit_codes() {
    assert_eq!(parse(&["--help"]), Ok(Parsed::Help));
    assert_eq!(parse(&["--bind", "x", "-h"]), Ok(Parsed::Help));
    assert!(USAGE.contains("Exit codes") && USAGE.contains("2 a flag or a value that is refused"));
}

#[test]
fn the_control_address_is_a_loopback_address_and_no_other() {
    // The one check the end-to-end depends on: a pod must not be able to ask for a token.
    for refused_address in [
        "0.0.0.0:0",
        "[::]:0",
        "172.18.0.1:0",
        "10.0.0.5:8080",
        "0.0.0.0:41234",
    ] {
        refused(
            &["--control", refused_address],
            "--control",
            &["loopback", refused_address],
        );
    }
    // Wherever it comes in the line, and whatever the issuer's own address is.
    refused(
        &[
            "--bind",
            "0.0.0.0:0",
            "--public-host",
            "172.18.0.1",
            "--control",
            "0.0.0.0:0",
        ],
        "--control",
        &["loopback"],
    );
    for allowed in ["127.0.0.1:0", "127.0.0.1:8080", "127.0.0.2:0", "[::1]:0"] {
        assert_eq!(
            run(&["--control", allowed]).control,
            allowed.parse().unwrap()
        );
    }
}

#[test]
fn a_control_address_that_is_not_an_address_is_refused() {
    for text in [
        "",
        "localhost:0",
        "127.0.0.1",
        "127.0.0.1:",
        ":0",
        "127.0.0.1:99999",
        "http://127.0.0.1:0",
    ] {
        refused(&["--control", text], "--control", &["address and a port"]);
    }
}

#[test]
fn a_bind_that_is_not_an_address_is_refused_before_anything_listens() {
    for text in [
        "",
        "0.0.0.0",
        "localhost:0",
        "999.1.1.1:0",
        "0.0.0.0:70000",
        "0.0.0.0:abc",
        "[::]",
        "http://0.0.0.0:0",
    ] {
        refused(&["--bind", text], "--bind", &["address and a port"]);
    }
    for allowed in ["0.0.0.0:0", "[::]:0", "127.0.0.1:41234", "192.168.1.5:8080"] {
        assert_eq!(run(&["--bind", allowed]).bind, allowed.parse().unwrap());
    }
}

#[test]
fn a_public_host_is_a_host_and_nothing_a_url_could_carry_more_of() {
    for text in [
        "",
        " ",
        "172.18.0.1:8080",
        "http://172.18.0.1",
        "172.18.0.1/",
        "172.18.0.1/path",
        "user@172.18.0.1",
        "999.1.1.1",
        "172.18.0.1.",
        ".example",
        "a..b",
        "-bad.example",
        "bad-.example",
        "a b",
        "a?b",
        "a#b",
        "::1",
        "[::1",
        "[not-an-address]",
        "[::1]:80",
        "[::1]/",
    ] {
        refused(
            &["--public-host", text],
            "--public-host",
            &["host", "no scheme"],
        );
    }
    for allowed in [
        "127.0.0.1",
        "172.18.0.1",
        "localhost",
        "issuer.example.test",
        "host_1.local",
        "[::1]",
        "[2001:db8::1]",
        "LOCALHOST",
    ] {
        assert_eq!(run(&["--public-host", allowed]).public_host, allowed);
    }
}

#[test]
fn a_flag_given_twice_is_refused_whether_or_not_the_values_agree() {
    for flag in [
        "--bind",
        "--public-host",
        "--control",
        "--audience",
        "--roles-claim",
    ] {
        let value = match flag {
            "--bind" | "--control" => "127.0.0.1:0",
            "--public-host" => "127.0.0.1",
            _ => "x",
        };
        refused(&[flag, value, flag, value], flag, &["twice"]);
        refused(
            &["--audience", "a", flag, value, flag, "other.example"],
            flag,
            &["twice"],
        );
    }
}

#[test]
fn a_missing_value_an_unknown_flag_and_a_stray_argument_are_refused() {
    for flag in [
        "--bind",
        "--public-host",
        "--control",
        "--audience",
        "--roles-claim",
    ] {
        refused(&[flag], flag, &["needs a value"]);
        // The next flag is not the value.
        refused(&[flag, "--audience", "a"], flag, &["needs a value"]);
    }
    refused(&["--port", "80"], "--port", &["unknown flag"]);
    refused(&["--bind=0.0.0.0:0"], "--bind=0.0.0.0:0", &["unknown flag"]);
    refused(&["-x"], "-x", &["unknown flag"]);
    refused(&["0.0.0.0:0"], "0.0.0.0:0", &["unexpected argument"]);
    refused(&["--audience", "a", "b"], "b", &["unexpected argument"]);
}

#[test]
fn a_blank_audience_and_an_empty_part_of_the_roles_claim_are_refused() {
    refused(&["--audience", ""], "--audience", &["blank"]);
    refused(&["--audience", "  "], "--audience", &["blank"]);
    for claim in ["", ".", "a..b", ".a", "a.", "a. .b"] {
        refused(&["--roles-claim", claim], "--roles-claim", &["none empty"]);
    }
    assert_eq!(run(&["--roles-claim", "roles"]).roles_claim, "roles");
}

#[tokio::test]
async fn an_address_that_cannot_be_listened_on_is_an_error_and_not_a_panic() {
    // Valid as a value, impossible as an address of this machine: what `try_start_at` is for.
    let taken = TestIdp::try_start_at("127.0.0.1:0", "127.0.0.1")
        .await
        .unwrap();
    let port = taken.issuer().rsplit(':').next().unwrap().to_owned();
    let again = TestIdp::try_start_at(&format!("127.0.0.1:{port}"), "127.0.0.1").await;
    assert!(again.is_err(), "a port in use was listened on twice");
    let foreign = TestIdp::try_start_at("203.0.113.9:0", "127.0.0.1").await;
    assert!(
        foreign.is_err(),
        "an address that is not this machine's was listened on"
    );
}
