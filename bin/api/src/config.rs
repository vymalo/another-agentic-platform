//! The configuration file: one YAML document, read at start, and refused whole when anything in it is wrong.
//!
//! **There is no secret in it.** The API checks tokens against the issuer's public keys and reaches the cluster with its
//! ServiceAccount; a secret in an agent is a reference to a Secret key. `deny_unknown_fields` at every level, so that a
//! misspelt key stops the API at start and does not leave a setting on its default.

use std::net::SocketAddr;
use std::path::Path;

use aap_auth_oidc::OidcConfig;
use aap_forms::Defaults;
use aap_platform_api::CorsOrigins;
use anyhow::{Context, Result, bail};
use serde::Deserialize;

const DEFAULT_LISTEN: &str = "0.0.0.0:8080";
const DEFAULT_HEALTH: &str = "0.0.0.0:8081";

fn listen() -> SocketAddr {
    DEFAULT_LISTEN
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 8080)))
}

fn health() -> SocketAddr {
    DEFAULT_HEALTH
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 8081)))
}

/// The file, as it is read. [`validate`](Self::validate) says whether the API can work with it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiConfig {
    /// Where the API listens.
    #[serde(default = "listen")]
    pub listen: SocketAddr,
    /// Where `/healthz` and `/readyz` are served.
    #[serde(default = "health")]
    pub health_listen: SocketAddr,
    /// Whose tokens are accepted.
    pub oidc: OidcConfig,
    /// Which browser origins may call the API.
    #[serde(default)]
    pub cors: CorsSection,
    /// What a new agent gets besides what its form sets; its `namespace` is the API's.
    pub defaults: Defaults,
}

/// The `cors` block.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorsSection {
    /// Exact origins, `scheme://host[:port]`. Empty: no CORS. A wildcard is refused.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

/// A configuration the API can work with.
#[derive(Debug)]
pub struct Settings {
    /// Where the API listens.
    pub listen: SocketAddr,
    /// Where the health endpoints are.
    pub health_listen: SocketAddr,
    /// The authenticator's configuration (validated).
    pub oidc: OidcConfig,
    /// The browser origins (validated).
    pub cors: CorsOrigins,
    /// The deployment's defaults (checked).
    pub defaults: Defaults,
}

impl ApiConfig {
    /// Read a file's text.
    ///
    /// # Errors
    ///
    /// The text is not YAML, or is not this file (a key it does not have, a value of the wrong type, a missing block).
    pub fn parse(text: &str) -> Result<Self> {
        serde_yaml::from_str(text).context("the configuration is not valid")
    }

    /// Whether the API can work with this file, and the settings if it can.
    ///
    /// # Errors
    ///
    /// The first setting that is wrong, named: an issuer that is not an `http(s)` URL, no audience, an origin that is a
    /// wildcard, a deployment default that would make the operator refuse every agent.
    pub fn validate(self) -> Result<Settings> {
        self.oidc.validate().context("oidc")?;
        let cors = CorsOrigins::new(&self.cors.allowed_origins).context("cors")?;
        let problems = self.defaults.check();
        if !problems.is_empty() {
            bail!(
                "defaults: the deployment's defaults are not usable: {}",
                problems.join("; ")
            );
        }
        Ok(Settings {
            listen: self.listen,
            health_listen: self.health_listen,
            oidc: self.oidc,
            cors,
            defaults: self.defaults,
        })
    }
}

/// Read, parse and validate the file at `path`.
///
/// # Errors
///
/// The file cannot be read, or [`ApiConfig::parse`] or [`ApiConfig::validate`] refuses it.
pub fn load(path: &Path) -> Result<Settings> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading the configuration file {}", path.display()))?;
    ApiConfig::parse(&text)
        .and_then(ApiConfig::validate)
        .with_context(|| format!("the configuration file {}", path.display()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const EXAMPLE: &str = include_str!("../config.example.yaml");

    fn example() -> serde_yaml::Value {
        serde_yaml::from_str(EXAMPLE).unwrap()
    }

    fn refused(value: &serde_yaml::Value) -> String {
        let text = serde_yaml::to_string(value).unwrap();
        format!(
            "{:#}",
            ApiConfig::parse(&text)
                .and_then(ApiConfig::validate)
                .unwrap_err()
        )
    }

    #[test]
    fn the_example_is_a_configuration_the_api_can_work_with() {
        let settings = ApiConfig::parse(EXAMPLE).unwrap().validate().unwrap();
        assert_eq!(settings.listen, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(settings.health_listen, "0.0.0.0:8081".parse().unwrap());
        assert_eq!(settings.defaults.namespace, "another-agentic-system");
        assert_eq!(
            settings.cors.as_slice(),
            ["https://agents.example.com", "tauri://localhost"]
        );
        assert_eq!(settings.oidc.audiences, ["another-agentic"]);
        assert_eq!(settings.oidc.roles_claim, "agentic_roles");
    }

    #[test]
    fn the_example_has_no_secret_in_it() {
        // The words a secret would be written under; a reference (`secretRef`, a Secret's name and key) is not one.
        let text = EXAMPLE.to_ascii_lowercase();
        for word in [
            "password",
            "clientsecret",
            "client_secret",
            "apikey:",
            "privatekey",
            "bearer ey",
        ] {
            assert!(!text.contains(word), "{word}");
        }
    }

    #[test]
    fn the_listen_addresses_have_defaults_and_the_rest_is_required() {
        let mut v = example();
        let map = v.as_mapping_mut().unwrap();
        map.remove("listen");
        map.remove("healthListen");
        map.remove("cors");
        let settings = ApiConfig::parse(&serde_yaml::to_string(&v).unwrap())
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(settings.listen.port(), 8080);
        assert_eq!(settings.health_listen.port(), 8081);
        assert!(settings.cors.is_empty(), "no cors block is no CORS");

        for required in ["oidc", "defaults"] {
            let mut v = example();
            v.as_mapping_mut().unwrap().remove(required);
            assert!(
                ApiConfig::parse(&serde_yaml::to_string(&v).unwrap()).is_err(),
                "{required}"
            );
        }
    }

    #[test]
    fn a_key_the_file_does_not_have_stops_the_api() {
        for path in [
            &[][..],
            &["oidc"],
            &["cors"],
            &["defaults"],
            &["defaults", "coder"],
        ] {
            let mut v = example();
            let mut at = &mut v;
            for key in path {
                at = at.get_mut(*key).unwrap();
            }
            at.as_mapping_mut()
                .unwrap()
                .insert("typo".into(), "x".into());
            let error = refused(&v);
            assert!(error.contains("typo"), "{path:?}: {error}");
        }
    }

    #[test]
    fn what_the_authenticator_cannot_work_with_is_refused() {
        let mut v = example();
        v["oidc"]["audiences"] = serde_yaml::from_str("[]").unwrap();
        assert!(refused(&v).contains("audience"), "{}", refused(&v));

        let mut v = example();
        v["oidc"]["audiences"] = serde_yaml::from_str("['']").unwrap();
        assert!(refused(&v).contains("audience"));

        let mut v = example();
        v["oidc"]["issuer"] = "keycloak.example.com".into();
        assert!(refused(&v).contains("issuer"));

        let mut v = example();
        v["oidc"]["rolesClaim"] = " ".into();
        assert!(refused(&v).contains("roles claim"));

        let mut v = example();
        v["oidc"].as_mapping_mut().unwrap().remove("rolesClaim");
        assert!(
            refused(&v).contains("rolesClaim"),
            "there is no default for the roles claim"
        );
    }

    #[test]
    fn a_wildcard_origin_is_refused_with_the_setting_it_is_in() {
        for origin in [
            "*",
            "https://*.example.com",
            "null",
            "https://agents.example.com/",
        ] {
            let mut v = example();
            v["cors"]["allowedOrigins"] = serde_yaml::from_str(&format!("['{origin}']")).unwrap();
            let error = refused(&v);
            assert!(error.contains("cors"), "{origin}: {error}");
        }
    }

    #[test]
    fn defaults_that_the_operator_would_refuse_are_refused_at_load() {
        let mut v = example();
        v["defaults"]["a2aBearer"]["key"] = "".into();
        let error = refused(&v);
        assert!(error.contains("defaults"), "{error}");
        let mut v = example();
        v["defaults"]["namespace"] = "Not A Namespace".into();
        assert!(refused(&v).contains("namespace"));
    }

    #[test]
    fn an_address_that_is_not_an_address_is_refused() {
        let mut v = example();
        v["listen"] = "everywhere".into();
        assert!(
            refused(&v).contains("listen") || refused(&v).contains("invalid"),
            "{}",
            refused(&v)
        );
    }

    #[test]
    fn a_file_that_is_not_there_says_which() {
        let error = format!(
            "{:#}",
            load(Path::new("/nonexistent/api.yaml")).unwrap_err()
        );
        assert!(error.contains("/nonexistent/api.yaml"), "{error}");
    }
}
