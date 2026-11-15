//! The exporter: one POST per batch, on the workspace's reqwest 0.12.

use std::fmt;

use aap_trace::{ExportError, FinishedSpan, Resource, SpanExporter};
use reqwest::header::{CONTENT_TYPE, HeaderValue};
use reqwest::redirect::Policy;
use reqwest::{Certificate, Client};
use serde_json::Value as Json;

use crate::encode::{encode, span_count};
use crate::headers::{Headers, MAX_HEADERS_FILE};
use crate::settings::{SettingsError, TraceSettings};

/// The most of an answer that is read.
const MAX_ANSWER: usize = 64 * 1024;

fn bad(variable: &'static str, reason: &'static str) -> SettingsError {
    SettingsError { variable, reason }
}

/// The OTLP/HTTP JSON exporter.
pub struct OtlpJson {
    client: Client,
    url: String,
    headers: Headers,
}

impl fmt::Debug for OtlpJson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No URL (it may name a private host) and no header.
        f.write_str("OtlpJson")
    }
}

impl OtlpJson {
    /// Read the files the settings name and build the client. Not a network call.
    ///
    /// The client follows no redirect, uses no proxy (the collector is in the cluster; `HTTPS_PROXY` is not read), and,
    /// when a certificate is given, trusts that bundle and nothing else.
    ///
    /// # Errors
    ///
    /// A file that cannot be read, is longer than 4 KiB (the headers) or does not parse; a certificate file that holds
    /// no certificate. The error names the variable and never the content.
    pub fn new(settings: &TraceSettings) -> Result<Self, SettingsError> {
        let headers = match &settings.headers_file {
            None => Headers::default(),
            Some(path) => {
                let var = "AAP_OTEL_HEADERS_FILE";
                let bytes = std::fs::read(path).map_err(|_| bad(var, "the file cannot be read"))?;
                if bytes.len() > MAX_HEADERS_FILE {
                    return Err(bad(var, "the file is longer than 4 KiB"));
                }
                let text =
                    std::str::from_utf8(&bytes).map_err(|_| bad(var, "the file is not text"))?;
                Headers::parse(text).map_err(|reason| bad(var, reason))?
            }
        };
        let mut builder = Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .timeout(settings.timeout)
            .user_agent("aap-trace-otlp");
        if let Some(path) = &settings.certificate {
            let var = "OTEL_EXPORTER_OTLP_CERTIFICATE";
            let pem = std::fs::read(path).map_err(|_| bad(var, "the file cannot be read"))?;
            let certs = Certificate::from_pem_bundle(&pem)
                .ok()
                .filter(|c| !c.is_empty())
                .ok_or(bad(var, "the file holds no PEM certificate"))?;
            builder = builder.tls_built_in_root_certs(false);
            for cert in certs {
                builder = builder.add_root_certificate(cert);
            }
        }
        let client = builder.build().map_err(|_| {
            bad(
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "the HTTP client cannot be built",
            )
        })?;
        Ok(Self {
            client,
            url: settings.endpoint.as_str().to_owned(),
            headers,
        })
    }
}

/// `partialSuccess.rejectedSpans` of an answer, which JSON mapping writes as a string or a number. The collector's
/// `errorMessage` is read by nobody.
fn rejected(body: &[u8]) -> usize {
    let Ok(json) = serde_json::from_slice::<Json>(body) else {
        return 0;
    };
    match &json["partialSuccess"]["rejectedSpans"] {
        Json::String(s) => s.parse().unwrap_or(0),
        Json::Number(n) => n
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
        _ => 0,
    }
}

impl SpanExporter for OtlpJson {
    async fn export(
        &self,
        resource: &Resource,
        batch: Vec<FinishedSpan>,
    ) -> Result<(), ExportError> {
        let body = encode(resource, &batch);
        if span_count(&body) == 0 {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&body).map_err(|_| ExportError::Failed)?;
        let mut response = self
            .client
            .post(&self.url)
            .headers(self.headers.map().clone())
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .body(bytes)
            .send()
            .await
            .map_err(|_| ExportError::Failed)?;
        if !response.status().is_success() {
            return Err(ExportError::Failed);
        }
        let mut answer = Vec::new();
        while let Ok(Some(chunk)) = response.chunk().await {
            if answer.len() + chunk.len() > MAX_ANSWER {
                answer.clear();
                break;
            }
            answer.extend_from_slice(&chunk);
        }
        match rejected(&answer) {
            0 => Ok(()),
            n => Err(ExportError::Rejected(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_reads_a_string_or_a_number_and_nothing_else() {
        assert_eq!(
            rejected(br#"{"partialSuccess":{"rejectedSpans":"3","errorMessage":"x"}}"#),
            3
        );
        assert_eq!(rejected(br#"{"partialSuccess":{"rejectedSpans":2}}"#), 2);
        assert_eq!(rejected(b"{}"), 0);
        assert_eq!(rejected(b"not json"), 0);
        assert_eq!(rejected(br#"{"partialSuccess":{"rejectedSpans":"x"}}"#), 0);
    }
}
