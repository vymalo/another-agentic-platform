//! Real keys, as an issuer may publish them: the leading zero octets of an Ed25519 key, of the coordinates of a
//! P-256 key and of the modulus and exponent of an RSA key (kept, cut, or covered by a sign byte), and the
//! tokens they sign. Over real HTTP, against a server that serves the key set a test builds.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_auth_oidc::testkit::{TEST_AUDIENCE, now};
use aap_auth_oidc::{OidcAuthenticator, OidcConfig};
use aap_ports::{AuthError, Authenticator, InvalidToken};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair as RsaKeyPair, KeySize, PublicKeyComponents};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, RSA_PKCS1_SHA256,
};
use axum::Router;
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{assert_accepted, assert_invalid, tuning};
use serde_json::{Value, json};

fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// An authenticator for an issuer whose key set is `keys`, and that issuer's name.
async fn issuer_with(keys: Vec<Value>) -> (OidcAuthenticator, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = json!({ "keys": keys });
    let router = Router::new().route(
        "/jwks",
        get(move || {
            let body = served.clone();
            async move { axum::Json(body) }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let config =
        OidcConfig::new(&base, [TEST_AUDIENCE], "roles").with_jwks_url(format!("{base}/jwks"));
    (
        OidcAuthenticator::with_tuning(config, tuning()).unwrap(),
        base,
    )
}

/// A token signed by `sign`, naming the key `kid`, good for `issuer`.
fn token(alg: &str, kid: &str, issuer: &str, sign: impl FnOnce(&[u8]) -> Vec<u8>) -> String {
    let header = json!({"alg": alg, "typ": "JWT", "kid": kid});
    let claims = json!({"iss": issuer, "aud": TEST_AUDIENCE, "sub": "alice", "exp": now() + 300});
    let input = format!(
        "{}.{}",
        b64(serde_json::to_vec(&header).unwrap()),
        b64(serde_json::to_vec(&claims).unwrap())
    );
    let signature = sign(input.as_bytes());
    format!("{input}.{}", b64(signature))
}

async fn accepted(auth: &OidcAuthenticator, token: &str, what: &str) {
    assert_accepted(auth.authenticate(Some(token)).await, what);
}

// ---- Ed25519 ---------------------------------------------------------------------------------------------------

/// An Ed25519 key whose public key starts with a zero octet (about one in 256).
fn ed25519_starting_with_zero() -> Ed25519KeyPair {
    for _ in 0..20_000 {
        let key = Ed25519KeyPair::generate().unwrap();
        if key.public_key().as_ref()[0] == 0 {
            return key;
        }
    }
    panic!("no Ed25519 key with a leading zero in 20000 tries");
}

#[tokio::test]
async fn an_ed25519_key_whose_public_key_starts_with_a_zero_octet_verifies() {
    let key = ed25519_starting_with_zero();
    let public = key.public_key().as_ref().to_vec();
    assert_eq!((public.len(), public[0]), (32, 0));
    let (auth, issuer) = issuer_with(vec![
        json!({"kty": "OKP", "crv": "Ed25519", "kid": "zero", "x": b64(&public)}),
    ])
    .await;
    let signed = token("EdDSA", "zero", &issuer, |m| key.sign(m).as_ref().to_vec());
    accepted(&auth, &signed, "an Ed25519 key with a leading zero").await;
}

#[tokio::test]
async fn an_ed25519_key_of_another_length_is_not_a_key() {
    // Raw octets, exactly 32: a zero octet is a part of the key, not padding to be taken off.
    let key = Ed25519KeyPair::generate().unwrap();
    let public = key.public_key().as_ref().to_vec();
    for (what, x) in [
        ("31 octets", public[1..].to_vec()),
        ("33 octets, a zero in front", [&[0u8][..], &public].concat()),
        ("none", Vec::new()),
    ] {
        let (auth, _) = issuer_with(vec![
            json!({"kty": "OKP", "crv": "Ed25519", "kid": "k", "x": b64(&x)}),
        ])
        .await;
        let error = auth.ready().await.unwrap_err();
        assert!(
            matches!(error, AuthError::Unavailable(_)),
            "{what}: {error:?}"
        );
    }
}

// ---- P-256 -------------------------------------------------------------------------------------------------------

struct Ec {
    key: EcdsaKeyPair,
    x: Vec<u8>,
    y: Vec<u8>,
}

impl Ec {
    /// A P-256 key whose `x` (or, with `in_y`, whose `y`) starts with a zero octet.
    fn starting_with_zero(in_y: bool) -> Self {
        for _ in 0..20_000 {
            let key = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
            let point = key.public_key().as_ref().to_vec();
            assert_eq!((point.len(), point[0]), (65, 4));
            let (x, y) = (point[1..33].to_vec(), point[33..65].to_vec());
            if (if in_y { y[0] } else { x[0] }) == 0 {
                return Self { key, x, y };
            }
        }
        panic!("no P-256 key with a leading zero in 20000 tries");
    }

    fn jwk(&self, x: &[u8], y: &[u8]) -> Value {
        json!({"kty": "EC", "crv": "P-256", "kid": "ec", "x": b64(x), "y": b64(y)})
    }

    fn token(&self, issuer: &str) -> String {
        token("ES256", "ec", issuer, |m| {
            self.key
                .sign(&SystemRandom::new(), m)
                .unwrap()
                .as_ref()
                .to_vec()
        })
    }
}

/// `bytes` without its leading zero octets.
fn trimmed(bytes: &[u8]) -> Vec<u8> {
    let zeros = bytes.iter().take_while(|b| **b == 0).count();
    bytes[zeros..].to_vec()
}

#[tokio::test]
async fn a_p256_coordinate_with_a_leading_zero_verifies_however_the_issuer_writes_it() {
    for in_y in [false, true] {
        let ec = Ec::starting_with_zero(in_y);
        let coordinate = if in_y { &ec.y } else { &ec.x };
        let other = if in_y { &ec.x } else { &ec.y };
        let signed_for = |issuer: &str| ec.token(issuer);
        let write = |c: &[u8]| {
            if in_y {
                ec.jwk(other, c)
            } else {
                ec.jwk(c, other)
            }
        };
        for (what, written) in [
            ("in full, 32 octets", coordinate.clone()),
            (
                "with the zeros cut, 31 octets or fewer",
                trimmed(coordinate),
            ),
            (
                "with a sign byte, 33 octets",
                [&[0u8][..], coordinate].concat(),
            ),
        ] {
            let (auth, issuer) = issuer_with(vec![write(&written)]).await;
            accepted(
                &auth,
                &signed_for(&issuer),
                &format!("{} {what}", if in_y { "y" } else { "x" }),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_p256_coordinate_that_cannot_be_one_is_not_a_key() {
    let ec = Ec::starting_with_zero(false);
    let too_long = [&[1u8][..], &ec.x].concat();
    for (what, jwk) in [
        ("33 significant octets", ec.jwk(&too_long, &ec.y)),
        ("an empty x", ec.jwk(&[], &ec.y)),
        ("an empty y", ec.jwk(&ec.x, &[])),
    ] {
        let (auth, _) = issuer_with(vec![jwk]).await;
        let error = auth.ready().await.unwrap_err();
        assert!(
            matches!(error, AuthError::Unavailable(_)),
            "{what}: {error:?}"
        );
    }
}

// ---- RSA ---------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_rsa_key_verifies_with_a_sign_byte_in_front_of_its_modulus_and_its_exponent() {
    let key = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
    let components = PublicKeyComponents::<Vec<u8>>::from(key.public_key());
    assert_eq!(components.n.len(), 256, "a 2048-bit modulus");
    let sign = |m: &[u8]| {
        let mut signature = vec![0; key.public_modulus_len()];
        key.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), m, &mut signature)
            .unwrap();
        signature
    };
    let with_zero = |bytes: &[u8]| [&[0u8][..], bytes].concat();
    for (what, n, e) in [
        (
            "as the key writes them",
            b64(&components.n),
            b64(&components.e),
        ),
        (
            "with a sign byte before n and e written AAEAAQ",
            b64(with_zero(&components.n)),
            "AAEAAQ".to_owned(),
        ),
        (
            "with a sign byte before n only",
            b64(with_zero(&components.n)),
            "AQAB".to_owned(),
        ),
    ] {
        let (auth, issuer) =
            issuer_with(vec![json!({"kty": "RSA", "kid": "rsa", "n": n, "e": e})]).await;
        accepted(&auth, &token("RS256", "rsa", &issuer, sign), what).await;
    }
}

#[tokio::test]
async fn an_rsa_modulus_under_2048_bits_is_not_made_big_enough_by_a_sign_byte() {
    // 255 significant octets with a zero in front is 256 octets long and still a 2040-bit modulus.
    let n = [&[0u8][..], &[0xC3u8; 255]].concat();
    let (auth, _) = issuer_with(vec![
        json!({"kty": "RSA", "kid": "small", "n": b64(&n), "e": "AQAB"}),
    ])
    .await;
    let error = auth.ready().await.unwrap_err();
    assert!(matches!(error, AuthError::Unavailable(_)), "{error:?}");
}

// ---- a kid that two keys share -----------------------------------------------------------------------------------

#[tokio::test]
async fn every_key_of_a_shared_kid_is_tried_and_not_only_the_first() {
    // A JWKS may hold two keys under one kid (a rotation that kept the name, two issuers merged): the token is
    // good if any of them signed it.
    let (first, second, stranger) = (
        Ed25519KeyPair::generate().unwrap(),
        Ed25519KeyPair::generate().unwrap(),
        Ed25519KeyPair::generate().unwrap(),
    );
    let jwk = |key: &Ed25519KeyPair| json!({"kty": "OKP", "crv": "Ed25519", "kid": "shared", "x": b64(key.public_key().as_ref())});
    let (auth, issuer) = issuer_with(vec![jwk(&first), jwk(&second)]).await;
    for (name, key) in [("the first", &first), ("the second", &second)] {
        let signed = token("EdDSA", "shared", &issuer, |m| {
            key.sign(m).as_ref().to_vec()
        });
        accepted(&auth, &signed, &format!("signed by {name} key of the kid")).await;
    }
    // A key that is not in the set is still a bad signature, and a token with no kid still needs one key only.
    let forged = token("EdDSA", "shared", &issuer, |m| {
        stranger.sign(m).as_ref().to_vec()
    });
    assert_invalid(
        auth.authenticate(Some(&forged)).await,
        InvalidToken::BadSignature,
        "signed by none of them",
    );
    let anonymous = format!(
        "{}.{}",
        b64(br#"{"alg":"EdDSA","typ":"JWT"}"#),
        forged.split_once('.').unwrap().1
    );
    assert_invalid(
        auth.authenticate(Some(&anonymous)).await,
        InvalidToken::UnknownKey,
        "no kid and two keys of the kind",
    );
}
