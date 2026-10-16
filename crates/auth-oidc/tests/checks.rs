//! What is checked of a token, one reason at a time: each word of `InvalidToken` is produced by the token that
//! deserves it, and the token that must be accepted is accepted. Over real HTTP, against an issuer on a local
//! port.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_auth_oidc::testkit::{
    ALGORITHMS, Alg, Signing, TEST_AUDIENCE, TEST_ROLES_CLAIM, now, set_path,
};
use aap_auth_oidc::{MAX_TOKEN_BYTES, OidcAuthenticator, OidcConfig};
use aap_ports::{AuthError, Authenticator, ErrorClass, InvalidToken};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{ROLES, Rig, SUBJECT, assert_accepted, assert_invalid, seconds_ago, tuning};
use serde_json::{Map, Value, json};

// ---- the algorithms ----------------------------------------------------------------------------------------

#[tokio::test]
async fn a_token_of_each_allowed_algorithm_is_accepted() {
    let rig = Rig::new().await;
    for alg in ALGORITHMS {
        let token = rig.mint(&rig.claims(), Signing::Published(alg));
        let principal = assert_accepted(rig.check(&token).await, &format!("{alg:?}"));
        assert_eq!(principal.subject(), SUBJECT, "{alg:?}");
        assert!(ROLES.iter().all(|role| principal.holds(role)), "{alg:?}");
    }
}

#[tokio::test]
async fn alg_none_is_refused_before_any_key_is_looked_at() {
    let rig = Rig::new().await;
    let token = rig.mint(&rig.claims(), Signing::NoneAlg);
    assert_invalid(rig.check(&token).await, InvalidToken::Malformed, "alg none");
    assert_eq!(
        rig.idp.jwks_fetches(),
        0,
        "no fetch for an algorithm that is not ours"
    );
}

#[tokio::test]
async fn hs256_signed_with_a_public_key_is_refused_as_an_algorithm_that_is_not_accepted() {
    let rig = Rig::new().await;
    // The keys are fetched first, so a refusal is not just "nothing to check against".
    assert_accepted(rig.check_claims(&rig.claims()).await, "a good token");
    let token = rig.mint(&rig.claims(), Signing::Hs256WithPublicKey);
    assert_invalid(
        rig.check(&token).await,
        InvalidToken::Malformed,
        "HS256 with the RSA public key as the secret",
    );
}

#[tokio::test]
async fn algorithms_that_are_not_allowed_are_refused_unread() {
    let rig = Rig::new().await;
    let kid = rig.idp.published_kid(Alg::Rs256);
    for alg in [
        "HS256", "HS384", "HS512", "RS512", "PS256", "PS384", "PS512", "ES384", "ES512", "none",
        "None", "rs256", "eddsa", "", "RS256 ",
    ] {
        let header = json!({"alg": alg, "typ": "JWT", "kid": kid});
        let token = rig.idp.forge(&header, &rig.claims(), b"not a signature");
        assert_invalid(rig.check(&token).await, InvalidToken::Malformed, alg);
    }
    // A header with no `alg`, and one that is not an object.
    for header in [json!({"typ": "JWT"}), json!([]), json!("RS256"), json!(7)] {
        let token = rig.idp.forge(&header, &rig.claims(), b"x");
        assert_invalid(
            rig.check(&token).await,
            InvalidToken::Malformed,
            &header.to_string(),
        );
    }
    assert_eq!(
        rig.idp.jwks_fetches(),
        0,
        "none of them made a request to the issuer"
    );
}

#[tokio::test]
async fn a_key_only_verifies_the_algorithms_of_its_kind() {
    // An RS256 token that names the kid of the P-256 key (and the converse) finds no key of its kind.
    let rig = Rig::new().await;
    let rsa_kid = rig.idp.published_kid(Alg::Rs256);
    let ec_kid = rig.idp.published_kid(Alg::Es256);
    for (alg, kid) in [
        ("RS256", &ec_kid),
        ("RS384", &ec_kid),
        ("ES256", &rsa_kid),
        ("EdDSA", &rsa_kid),
    ] {
        let token = rig.idp.forge(
            &json!({"alg": alg, "typ": "JWT", "kid": kid}),
            &rig.claims(),
            &[7u8; 64],
        );
        assert_invalid(rig.check(&token).await, InvalidToken::UnknownKey, alg);
    }
}

// ---- the signature and the key -----------------------------------------------------------------------------

#[tokio::test]
async fn a_signature_by_another_key_under_a_published_kid_is_a_bad_signature() {
    let rig = Rig::new().await;
    for alg in ALGORITHMS {
        let token = rig.mint(&rig.claims(), Signing::UnpublishedUnderPublishedKid(alg));
        assert_invalid(
            rig.check(&token).await,
            InvalidToken::BadSignature,
            &format!("{alg:?}"),
        );
    }
}

#[tokio::test]
async fn a_payload_edited_after_signing_is_a_bad_signature() {
    let rig = Rig::new().await;
    for alg in ALGORITHMS {
        let good = rig.mint(&rig.claims(), Signing::Published(alg));
        let parts: Vec<&str> = good.split('.').collect();
        // The edit a person would make: a permission more.
        let mut claims = rig.claims();
        claims.insert(
            TEST_ROLES_CLAIM.to_owned(),
            json!([
                "platform:agents.read",
                "platform:agents.write",
                "platform:secrets.pick"
            ]),
        );
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let tampered = format!("{}.{payload}.{}", parts[0], parts[2]);
        assert_invalid(
            rig.check(&tampered).await,
            InvalidToken::BadSignature,
            &format!("{alg:?}"),
        );
        // And the signature of another token of the same key.
        let other = rig.mint(
            &rig.idp.claims(TEST_AUDIENCE, "someone-else"),
            Signing::Published(alg),
        );
        let swapped = format!(
            "{}.{}.{}",
            parts[0],
            parts[1],
            other.split('.').nth(2).unwrap()
        );
        assert_invalid(
            rig.check(&swapped).await,
            InvalidToken::BadSignature,
            &format!("{alg:?} swapped"),
        );
    }
}

#[tokio::test]
async fn an_unknown_kid_is_an_unknown_key() {
    let rig = Rig::new().await;
    for alg in ALGORITHMS {
        let token = rig.mint(&rig.claims(), Signing::Unpublished(alg));
        assert_invalid(
            rig.check(&token).await,
            InvalidToken::UnknownKey,
            &format!("{alg:?}"),
        );
    }
}

#[tokio::test]
async fn a_token_with_no_kid_takes_the_only_key_of_its_kind() {
    let rig = Rig::new().await;
    for alg in ALGORITHMS {
        let token = rig.mint(&rig.claims(), Signing::PublishedWithoutKid(alg));
        assert_accepted(rig.check(&token).await, &format!("{alg:?} without a kid"));
    }
    // Once the copy holds a second key of each kind, no kid names none of them: not a guess.
    rig.idp.publish_unpublished_keys();
    rig.wait_out_the_interval().await;
    let rotated = rig.mint(&rig.claims(), Signing::Unpublished(Alg::Rs256));
    assert_accepted(
        rig.check(&rotated).await,
        "the rotated key, found by its kid",
    );
    for alg in ALGORITHMS {
        let token = rig.mint(&rig.claims(), Signing::PublishedWithoutKid(alg));
        assert_invalid(
            rig.check(&token).await,
            InvalidToken::UnknownKey,
            &format!("{alg:?} without a kid, two keys"),
        );
    }
}

// ---- the times -----------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_token_past_its_exp_is_expired_beyond_the_leeway_of_60_seconds() {
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    for (ago, what) in [(3600, "an hour ago"), (120, "two minutes ago")] {
        claims.insert("exp".into(), json!(seconds_ago(ago)));
        assert_invalid(rig.check_claims(&claims).await, InvalidToken::Expired, what);
    }
    claims.insert("exp".into(), json!(seconds_ago(30)));
    assert_accepted(
        rig.check_claims(&claims).await,
        "expired 30 s ago, inside the leeway",
    );
    claims.insert("exp".into(), json!(now() + 3600));
    assert_accepted(rig.check_claims(&claims).await, "valid for an hour");
}

#[tokio::test]
async fn a_token_before_its_nbf_is_refused_beyond_the_leeway_of_60_seconds() {
    // The closed set has no word for "not yet": the nearest is `Expired`, a token outside its time.
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    claims.insert("nbf".into(), json!(now() + 3600));
    assert_invalid(
        rig.check_claims(&claims).await,
        InvalidToken::Expired,
        "valid in an hour",
    );
    claims.insert("nbf".into(), json!(now() + 30));
    assert_accepted(
        rig.check_claims(&claims).await,
        "valid in 30 s, inside the leeway",
    );
    claims.insert("nbf".into(), json!(seconds_ago(10)));
    assert_accepted(rig.check_claims(&claims).await, "valid since 10 s");
    claims.insert("nbf".into(), json!("tomorrow"));
    assert_invalid(
        rig.check_claims(&claims).await,
        InvalidToken::Malformed,
        "an nbf that is not a time",
    );
}

#[tokio::test]
async fn exp_is_required_and_must_be_a_time() {
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    claims.remove("exp");
    assert_invalid(
        rig.check_claims(&claims).await,
        InvalidToken::MissingClaim,
        "no exp",
    );
    for not_a_time in [json!("soon"), json!(null), json!(true), json!(-1)] {
        claims.insert("exp".into(), not_a_time.clone());
        assert_invalid(
            rig.check_claims(&claims).await,
            InvalidToken::MissingClaim,
            &format!("exp {not_a_time}"),
        );
    }
    // The library cannot read on past a list or an object where it wanted a time: the token is malformed.
    for not_a_time in [json!([1]), json!({"a": 1})] {
        claims.insert("exp".into(), not_a_time.clone());
        assert_invalid(
            rig.check_claims(&claims).await,
            InvalidToken::Malformed,
            &format!("exp {not_a_time}"),
        );
    }
}

#[tokio::test]
async fn iat_is_not_required() {
    // §60a names `exp` only; the system's orchestrator also requires `iat` (decision 2 of D6).
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    claims.remove("iat");
    assert_accepted(rig.check_claims(&claims).await, "no iat");
}

// ---- the issuer, the audience, the subject -------------------------------------------------------------------

#[tokio::test]
async fn the_issuer_must_match_exactly() {
    let rig = Rig::new().await;
    let issuer = rig.idp.issuer();
    for wrong in [
        format!("{issuer}/"),
        issuer.to_uppercase(),
        issuer.chars().take(issuer.chars().count() - 1).collect(),
        "https://evil.example".to_owned(),
        String::new(),
    ] {
        let mut claims = rig.claims();
        claims.insert("iss".into(), json!(wrong));
        assert_invalid(
            rig.check_claims(&claims).await,
            InvalidToken::WrongIssuer,
            &format!("issuer {wrong:?}"),
        );
    }
    let mut claims = rig.claims();
    claims.remove("iss");
    assert_invalid(
        rig.check_claims(&claims).await,
        InvalidToken::WrongIssuer,
        "no issuer",
    );
    claims.insert("iss".into(), json!(7));
    assert_invalid(
        rig.check_claims(&claims).await,
        InvalidToken::WrongIssuer,
        "an issuer that is not text",
    );
}

#[tokio::test]
async fn one_of_the_audiences_must_be_in_aud() {
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    for (aud, what) in [
        (json!("someone-else"), "another audience"),
        (json!([]), "an empty list of audiences"),
        (json!(["someone-else", "another"]), "a list without it"),
        (json!(""), "an empty audience"),
        (json!(7), "an audience that is not text"),
        (json!({"a": TEST_AUDIENCE}), "an object"),
    ] {
        claims.insert("aud".into(), aud);
        assert_invalid(
            rig.check_claims(&claims).await,
            InvalidToken::WrongAudience,
            what,
        );
    }
    claims.remove("aud");
    assert_invalid(
        rig.check_claims(&claims).await,
        InvalidToken::WrongAudience,
        "no audience",
    );
    claims.insert("aud".into(), json!(["someone-else", TEST_AUDIENCE]));
    assert_accepted(rig.check_claims(&claims).await, "a list with it");
    claims.insert("aud".into(), json!(TEST_AUDIENCE));
    assert_accepted(rig.check_claims(&claims).await, "the audience alone");
}

#[tokio::test]
async fn any_of_several_configured_audiences_is_enough() {
    let rig = Rig::with(
        |mut config| {
            config.audiences = vec!["api-a".into(), "api-b".into()];
            config
        },
        |tuning| tuning,
    )
    .await;
    let mut claims = rig.claims();
    for (aud, accepted) in [
        (json!("api-a"), true),
        (json!("api-b"), true),
        (json!(["x", "api-b"]), true),
        (json!("api-c"), false),
        (json!(["api-c", "api-d"]), false),
        (json!(TEST_AUDIENCE), false),
    ] {
        claims.insert("aud".into(), aud.clone());
        let result = rig.check_claims(&claims).await;
        if accepted {
            assert_accepted(result, &aud.to_string());
        } else {
            assert_invalid(result, InvalidToken::WrongAudience, &aud.to_string());
        }
    }
}

#[tokio::test]
async fn sub_is_required_and_is_the_subject() {
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    claims.remove("sub");
    assert_invalid(
        rig.check_claims(&claims).await,
        InvalidToken::MissingClaim,
        "no sub",
    );
    for bad in [json!(""), json!(5), json!(null), json!(true)] {
        claims.insert("sub".into(), bad.clone());
        assert_invalid(
            rig.check_claims(&claims).await,
            InvalidToken::MissingClaim,
            &format!("sub {bad}"),
        );
    }
    // The library cannot read on past a list or an object where it wanted text: the token is malformed.
    for bad in [json!(["a"]), json!({"a": 1})] {
        claims.insert("sub".into(), bad.clone());
        assert_invalid(
            rig.check_claims(&claims).await,
            InvalidToken::Malformed,
            &format!("sub {bad}"),
        );
    }
    claims.insert("sub".into(), json!("Some Subject-42"));
    let principal = assert_accepted(rig.check_claims(&claims).await, "a sub");
    assert_eq!(principal.subject(), "Some Subject-42");
}

#[tokio::test]
async fn a_token_bound_to_a_key_is_accepted_as_a_plain_bearer() {
    // DPoP is not in D6: a token with `cnf` is a bearer token here (decision 3 of D6, a question for the owner).
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    claims.insert(
        "cnf".into(),
        json!({"jkt": "0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I"}),
    );
    assert_accepted(rig.check_claims(&claims).await, "cnf.jkt");
}

#[tokio::test]
async fn the_time_checks_come_after_the_signature() {
    // An expired token signed by nobody is a bad signature, not "expired": the claims of a token that is not
    // the issuer's are not read as if they meant something.
    let rig = Rig::new().await;
    let mut claims = rig.claims();
    claims.insert("exp".into(), json!(seconds_ago(3600)));
    let token = rig.mint(&claims, Signing::UnpublishedUnderPublishedKid(Alg::Rs256));
    assert_invalid(
        rig.check(&token).await,
        InvalidToken::BadSignature,
        "expired and forged",
    );
}

// ---- size and shape ------------------------------------------------------------------------------------------

/// A token with a padding claim of `pad` characters.
fn padded(rig: &Rig, pad: usize) -> String {
    let mut claims = rig.claims();
    claims.insert("pad".into(), json!("x".repeat(pad)));
    rig.mint(&claims, Signing::Published(Alg::Rs256))
}

#[tokio::test]
async fn a_token_of_16_kib_is_read_and_one_byte_more_is_not() {
    let rig = Rig::new().await;
    // The length grows with the padding in steps of at most two: find the last padding that fits.
    let (mut low, mut high) = (0, MAX_TOKEN_BYTES);
    while low + 1 < high {
        let mid = (low + high) / 2;
        if padded(&rig, mid).len() <= MAX_TOKEN_BYTES {
            low = mid;
        } else {
            high = mid;
        }
    }
    let largest = padded(&rig, low);
    assert!(
        largest.len() <= MAX_TOKEN_BYTES && largest.len() + 2 >= MAX_TOKEN_BYTES,
        "{}",
        largest.len()
    );
    assert_accepted(rig.check(&largest).await, "the largest token read");
    let too_large = padded(&rig, high);
    assert!(too_large.len() > MAX_TOKEN_BYTES);
    assert_invalid(
        rig.check(&too_large).await,
        InvalidToken::Malformed,
        "one step more",
    );
}

#[tokio::test]
async fn a_token_over_the_cap_is_refused_without_a_fetch() {
    let rig = Rig::new().await;
    let huge = "a".repeat(MAX_TOKEN_BYTES + 1);
    assert_invalid(
        rig.check(&huge).await,
        InvalidToken::Malformed,
        "garbage over the cap",
    );
    assert_eq!(rig.idp.jwks_fetches(), 0);
}

#[tokio::test]
async fn what_is_not_a_token_is_malformed_and_makes_no_fetch() {
    let rig = Rig::new().await;
    let good = rig.mint(&rig.claims(), Signing::Published(Alg::Rs256));
    let garbage = [
        " ".to_owned(),
        "garbage".to_owned(),
        "a.b".to_owned(),
        "a.b.c".to_owned(),
        "..".to_owned(),
        "...".to_owned(),
        format!("Bearer {good}"),
        format!("{good}.extra"),
        "e30.e30.".to_owned(),
        // A header that is not JSON, one that is not an object, one with no algorithm.
        "bm9wZQ.e30.AAAA".to_owned(),
        "W10.e30.AAAA".to_owned(),
        "e30.e30.AAAA".to_owned(),
        "a".repeat(64 * 1024),
    ];
    for token in garbage {
        let shown: String = token.chars().take(24).collect();
        assert_invalid(
            rig.check(&token).await,
            InvalidToken::Malformed,
            &format!("{shown:?}"),
        );
    }
    assert_eq!(rig.idp.jwks_fetches(), 0, "none of it reached the issuer");
}

#[tokio::test]
async fn a_signature_that_cannot_be_read_is_malformed_and_one_that_does_not_verify_is_not() {
    let rig = Rig::new().await;
    let good = rig.mint(&rig.claims(), Signing::Published(Alg::Rs256));
    let (signing_input, _) = good.rsplit_once('.').unwrap();
    for signature in ["!!!", "a b", "%%%%", "AAAA===="] {
        // Past the header, so the key was looked at; the signature is not base64url.
        let token = format!("{signing_input}.{signature}");
        assert_invalid(rig.check(&token).await, InvalidToken::Malformed, signature);
    }
    for signature in ["", "AAAA"] {
        let token = format!("{signing_input}.{signature}");
        assert_invalid(
            rig.check(&token).await,
            InvalidToken::BadSignature,
            signature,
        );
    }
    // White space around a token is not a token: the router strips the scheme, this strips nothing.
    assert_invalid(
        rig.check(&format!("{good}\n")).await,
        InvalidToken::Malformed,
        "a newline after",
    );
    assert_invalid(
        rig.check(&format!(" {good}")).await,
        InvalidToken::Malformed,
        "a space before",
    );
}

#[tokio::test]
async fn no_token_is_missing_and_a_refusal_is_invalid_not_transient() {
    let rig = Rig::new().await;
    for token in [None, Some("")] {
        let error = rig.auth.authenticate(token).await.unwrap_err();
        assert!(matches!(error, AuthError::Missing), "{token:?}: {error:?}");
    }
    let refused = rig.check("garbage").await.unwrap_err();
    assert_eq!(aap_ports::Classify::class(&refused), ErrorClass::Invalid);
    assert_eq!(rig.idp.jwks_fetches(), 0, "no token needs no keys");
}

// ---- the roles claim -----------------------------------------------------------------------------------------

/// An authenticator whose roles claim is `path`.
async fn rig_for(path: &str) -> Rig {
    let path = path.to_owned();
    Rig::with(
        move |mut config| {
            config.roles_claim = path;
            config
        },
        |tuning| tuning,
    )
    .await
}

fn roles_of(principal: &aap_ports::Principal) -> Vec<String> {
    principal.permissions().map(str::to_owned).collect()
}

#[tokio::test]
async fn the_roles_are_the_values_at_the_configured_path_and_a_dotted_path_goes_through_objects() {
    let rig = rig_for("resource_access.another-agentic.roles").await;
    let mut claims = rig.idp.claims(TEST_AUDIENCE, SUBJECT);
    set_path(
        &mut claims,
        "resource_access.another-agentic.roles",
        json!(["platform:agents.read", "user"]),
    );
    // Another client's roles are not ours.
    set_path(
        &mut claims,
        "resource_access.other-client.roles",
        json!(["platform:agents.write"]),
    );
    claims.insert(TEST_ROLES_CLAIM.into(), json!(["platform:models.write"]));
    let principal = assert_accepted(rig.check_claims(&claims).await, "nested");
    assert_eq!(roles_of(&principal), ["platform:agents.read", "user"]);
}

#[tokio::test]
async fn a_claim_whose_own_name_has_dots_is_found_by_that_name_first() {
    let rig = rig_for("https://example.com/roles").await;
    let mut claims = rig.idp.claims(TEST_AUDIENCE, SUBJECT);
    claims.insert(
        "https://example.com/roles".into(),
        json!(["from-the-exact-name"]),
    );
    // A nested claim that the path could also reach is not what the name says.
    set_path(
        &mut claims,
        "https://example",
        json!({"com/roles": ["nested"]}),
    );
    let principal = assert_accepted(rig.check_claims(&claims).await, "a namespaced claim");
    assert_eq!(roles_of(&principal), ["from-the-exact-name"]);

    let rig = rig_for("a.b").await;
    let mut claims = rig.idp.claims(TEST_AUDIENCE, SUBJECT);
    claims.insert("a.b".into(), json!(["flat"]));
    claims.insert("a".into(), json!({"b": ["nested"]}));
    let principal = assert_accepted(rig.check_claims(&claims).await, "flat before nested");
    assert_eq!(roles_of(&principal), ["flat"]);
}

#[tokio::test]
async fn a_text_is_one_role_and_a_set_has_each_value_once() {
    let rig = Rig::new().await;
    let mut claims = rig.idp.claims(TEST_AUDIENCE, SUBJECT);
    claims.insert(TEST_ROLES_CLAIM.into(), json!("solo"));
    assert_eq!(
        roles_of(&assert_accepted(rig.check_claims(&claims).await, "a text")),
        ["solo"]
    );
    claims.insert(TEST_ROLES_CLAIM.into(), json!(["b", "a", "b", "A"]));
    assert_eq!(
        roles_of(&assert_accepted(rig.check_claims(&claims).await, "a list")),
        ["A", "a", "b"],
        "a set in byte order, compared whole"
    );
}

#[tokio::test]
async fn a_bad_roles_claim_is_no_permission_and_never_refuses_the_token() {
    let rig = Rig::new().await;
    let mut claims = rig.idp.claims(TEST_AUDIENCE, SUBJECT);
    for (what, value) in [
        ("a number", json!(3)),
        ("a boolean", json!(true)),
        ("null", json!(null)),
        ("an object", json!({"platform:agents.read": true})),
        ("an empty list", json!([])),
        ("an empty text", json!("")),
        (
            "a list of things that are not texts",
            json!([1, null, {"a": 1}, [2], true]),
        ),
    ] {
        claims.insert(TEST_ROLES_CLAIM.into(), value);
        let principal = assert_accepted(rig.check_claims(&claims).await, what);
        assert_eq!(principal.subject(), SUBJECT, "{what}");
        assert_eq!(principal.permissions().count(), 0, "{what}");
    }
    // Texts and others: the texts stay.
    claims.insert(
        TEST_ROLES_CLAIM.into(),
        json!(["a", 7, null, {"x": 1}, "", "c"]),
    );
    let principal = assert_accepted(rig.check_claims(&claims).await, "texts and others");
    assert_eq!(roles_of(&principal), ["a", "c"]);
    // No claim at all.
    claims.remove(TEST_ROLES_CLAIM);
    let principal = assert_accepted(rig.check_claims(&claims).await, "no claim");
    assert_eq!(principal.permissions().count(), 0);
    // The path goes through something that is not an object.
    let rig = rig_for("realm_access.roles").await;
    for realm in [json!("text"), json!(["roles"]), json!(null), json!(5)] {
        let mut claims = rig.idp.claims(TEST_AUDIENCE, SUBJECT);
        claims.insert("realm_access".into(), realm);
        let principal = assert_accepted(
            rig.check_claims(&claims).await,
            "a path through a non-object",
        );
        assert_eq!(principal.permissions().count(), 0);
    }
}

#[tokio::test]
async fn only_the_configured_claim_is_read() {
    // `scope`, `groups` and the like hold things that are not permissions: they are not read.
    let rig = Rig::new().await;
    let mut claims = rig.idp.claims(TEST_AUDIENCE, SUBJECT);
    claims.insert("scope".into(), json!("platform:agents.write"));
    claims.insert("groups".into(), json!(["platform:agents.write"]));
    claims.insert("roles".into(), json!(["platform:agents.write"]));
    set_path(
        &mut claims,
        "realm_access.roles",
        json!(["platform:agents.write"]),
    );
    let principal = assert_accepted(rig.check_claims(&claims).await, "other claims");
    assert_eq!(principal.permissions().count(), 0);
}

// ---- a smoke check of the whole shape ------------------------------------------------------------------------

#[tokio::test]
async fn the_authenticator_is_cheap_to_clone_and_its_futures_can_be_spawned() {
    let rig = Rig::new().await;
    let token = rig.mint(&rig.claims(), Signing::Published(Alg::Es256));
    let auth: OidcAuthenticator = rig.auth.clone();
    let principal = tokio::spawn(async move { auth.authenticate(Some(&token)).await })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(principal.subject(), SUBJECT);
    // Clones share the keys: one fetch.
    assert_eq!(rig.idp.jwks_fetches(), 1);
    let _: OidcConfig = rig.idp.config();
    let _ = tuning();
    let _: &Map<String, Value> = &rig.claims();
}
