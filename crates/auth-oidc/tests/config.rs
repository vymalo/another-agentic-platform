//! The configuration: what a YAML file may say, and what is refused at construction (nothing is fetched to
//! find out).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use aap_auth_oidc::{ConfigError, OidcAuthenticator, OidcConfig};

fn parse(yaml: &str) -> Result<OidcConfig, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

const FULL: &str = "
issuer: https://keycloak.example/realms/another-agentic
audiences: [another-agentic, another-agentic-desktop]
rolesClaim: agentic_roles
jwksUrl: https://keycloak.example/realms/another-agentic/protocol/openid-connect/certs
";

#[test]
fn a_file_is_read_with_the_names_of_the_platform_apis_configuration() {
    let config = parse(FULL).unwrap();
    assert_eq!(
        config.issuer,
        "https://keycloak.example/realms/another-agentic"
    );
    assert_eq!(
        config.audiences,
        ["another-agentic", "another-agentic-desktop"]
    );
    assert_eq!(config.roles_claim, "agentic_roles");
    assert_eq!(
        config.jwks_url.as_deref(),
        Some("https://keycloak.example/realms/another-agentic/protocol/openid-connect/certs")
    );
    config.validate().unwrap();
    OidcAuthenticator::new(config).unwrap();

    let minimal = parse(
        "{ issuer: 'https://idp.example', audiences: [api], rolesClaim: realm_access.roles }",
    )
    .unwrap();
    assert_eq!(minimal.jwks_url, None);
    minimal.validate().unwrap();
}

#[test]
fn a_file_that_is_missing_something_or_says_more_is_refused() {
    // No default for the roles claim or the audiences: the API would run with nobody holding anything, or
    // with the wrong audience.
    for yaml in [
        "{ audiences: [api], rolesClaim: r }",
        "{ issuer: 'https://idp.example', rolesClaim: r }",
        "{ issuer: 'https://idp.example', audiences: [api] }",
        // A misspelt key is not ignored.
        "{ issuer: 'https://idp.example', audiences: [api], rolesClaim: r, audience: api }",
        "{ issuer: 'https://idp.example', audiences: [api], rolesClaim: r, jwks_url: 'https://x' }",
        // A single audience is a list.
        "{ issuer: 'https://idp.example', audiences: api, rolesClaim: r }",
    ] {
        assert!(parse(yaml).is_err(), "{yaml}");
    }
}

#[test]
fn a_configuration_that_cannot_work_is_refused_at_construction() {
    let ok = || OidcConfig::new("https://idp.example/realms/x", ["api"], "roles");
    assert!(OidcAuthenticator::new(ok()).is_ok());

    for (issuer, why) in [
        ("ftp://idp.example", "scheme"),
        ("idp.example", "no scheme"),
        ("https://user:pw@idp.example", "credentials"),
        ("https://idp.example/?a=b", "query"),
        ("https://idp.example/#f", "fragment"),
        ("", "empty"),
        (" https://idp.example", "white space before"),
        ("https://idp.example ", "white space after"),
        ("file:///etc/passwd", "a file"),
    ] {
        let mut config = ok();
        config.issuer = issuer.to_owned();
        let error = OidcAuthenticator::new(config).unwrap_err();
        assert!(matches!(error, ConfigError::Issuer), "{why}: {error:?}");
    }
    let none: [&str; 0] = [];
    for audiences in [
        Vec::<String>::new(),
        vec![" ".into()],
        vec!["api".into(), String::new()],
    ] {
        let mut config = ok();
        config.audiences = audiences;
        assert!(matches!(
            config.validate().unwrap_err(),
            ConfigError::Audiences
        ));
    }
    assert!(matches!(
        OidcConfig::new("https://idp.example", none, "r")
            .validate()
            .unwrap_err(),
        ConfigError::Audiences
    ));
    for claim in ["", "  "] {
        let mut config = ok();
        config.roles_claim = claim.to_owned();
        assert!(matches!(
            config.validate().unwrap_err(),
            ConfigError::RolesClaim
        ));
    }
    for jwks in [
        "file:///etc/passwd",
        "/jwks",
        "https://u:p@idp.example/jwks",
        " https://idp.example/jwks",
    ] {
        let error = ok().with_jwks_url(jwks).validate().unwrap_err();
        assert!(matches!(error, ConfigError::JwksUrl), "{jwks}: {error:?}");
    }
    // An http issuer is allowed: an identity provider in the cluster is often reached that way.
    OidcConfig::new("http://keycloak.keycloak.svc:8080/realms/x", ["api"], "r")
        .validate()
        .unwrap();
}

#[test]
fn a_refusal_names_the_setting_and_never_shows_its_value() {
    let mut config = OidcConfig::new("https://user:hunter2@idp.example", ["api"], "r");
    let shown = config.validate().unwrap_err().to_string();
    assert!(
        shown.contains("issuer") && !shown.contains("hunter2"),
        "{shown}"
    );
    config.issuer = "https://idp.example".into();
    config.jwks_url = Some("https://user:hunter2@idp.example/jwks".into());
    let error = config.validate().unwrap_err();
    assert!(!format!("{error} {error:?}").contains("hunter2"));
}
