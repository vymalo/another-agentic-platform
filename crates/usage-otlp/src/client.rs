//! The HTTP client, as `aap-trace-otlp`'s: no redirect, no proxy, a given CA bundle as the only trust.

use std::path::Path;

use reqwest::redirect::Policy;
use reqwest::{Certificate, Client};

use crate::settings::{SettingsError, UsageOtlpSettings};

fn bad(setting: &'static str, reason: &'static str) -> SettingsError {
    SettingsError { setting, reason }
}

pub(crate) fn build(settings: &UsageOtlpSettings) -> Result<Client, SettingsError> {
    let mut builder = Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .timeout(settings.timeout)
        .user_agent("aap-usage-otlp");
    if let Some(path) = &settings.ca {
        builder = trust_only(builder, path)?;
    }
    builder
        .build()
        .map_err(|_| bad("endpoint", "the HTTP client cannot be built"))
}

fn trust_only(
    mut builder: reqwest::ClientBuilder,
    path: &Path,
) -> Result<reqwest::ClientBuilder, SettingsError> {
    let pem = std::fs::read(path).map_err(|_| bad("ca", "the file cannot be read"))?;
    let certs = Certificate::from_pem_bundle(&pem)
        .ok()
        .filter(|c| !c.is_empty())
        .ok_or(bad("ca", "the file holds no PEM certificate"))?;
    builder = builder.tls_built_in_root_certs(false);
    for cert in certs {
        builder = builder.add_root_certificate(cert);
    }
    Ok(builder)
}
