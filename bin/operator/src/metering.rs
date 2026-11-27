//! The pod-minute meter's composition (ANR-1103, AD-069): the sink that carries a line to the collector, the sources that
//! say whose minute it is (the broker for a run pod, the `AgentService` for an agent's own pod: AD-072), and the meter's
//! settings, built from `metering` of the file.
//!
//! **Fail closed.** The bearer is read now: a missing or empty file, an unset or empty variable is a refusal of the file
//! ([`ReferenceRefused`](crate::config::ReferenceRefused), exit 78), never a meter that asks with no bearer. A CA bundle that
//! cannot be read is refused the same way. **Nothing here logs a run id, an account, a URL or a token.**

use std::sync::Arc;
use std::time::Duration;

use aap_controller::MeterConfig;
use aap_trace_otlp::SecretRef;
use aap_usage::{AccountSource, AccountUnavailable, PodMinuteEvent, PodRef, UsageError, UsageSink};
use aap_usage_broker::{BrokerAccounts, Token};
use aap_usage_otlp::OtlpUsageSink;
use anyhow::Result;

use crate::config::{Metering, ReferenceRefused};

/// How long the lines still queued have to reach the collector when the operator stops, inside the 30 s of
/// `terminationGracePeriodSeconds` (the traces' own flush is 3 s).
pub const FLUSH: Duration = Duration::from_secs(5);

/// The sink behind a pointer the operator keeps, so that it can be shut down (flushed) when the operator stops.
#[derive(Clone)]
pub struct SharedSink(pub Arc<OtlpUsageSink>);

impl UsageSink for SharedSink {
    async fn send(&self, event: PodMinuteEvent) -> Result<(), UsageError> {
        self.0.send(event).await
    }

    fn outcomes(&self) -> Option<Vec<aap_usage::Outcome>> {
        self.0.outcomes()
    }
}

/// The two account sources, composed by pod kind (AD-072): a run pod is the `run` source's to answer, an agent's own pod
/// the `agent` source's, and neither is asked about the other kind.
pub struct MeterAccounts<R, A> {
    run: R,
    agent: A,
}

impl<R, A> MeterAccounts<R, A> {
    /// Routes a [`PodRef::Run`] to `run` and a [`PodRef::Agent`] to `agent`.
    pub fn new(run: R, agent: A) -> Self {
        Self { run, agent }
    }
}

impl<R: AccountSource, A: AccountSource> AccountSource for MeterAccounts<R, A> {
    async fn account(&self, pod: &PodRef) -> Result<Option<String>, AccountUnavailable> {
        match pod {
            PodRef::Run { .. } => self.run.account(pod).await,
            PodRef::Agent { .. } => self.agent.account(pod).await,
        }
    }
}

/// What the operator hands to `Operator::with_meter` and `with_meter_config`.
pub struct Parts {
    /// The collector sink; the caller shuts it down on the way out.
    pub sink: SharedSink,
    /// The broker, for a run pod. The caller composes it with the agent source ([`MeterAccounts`]) once it has the cluster
    /// client: this is built before the cluster is reached, so a bad bearer stops the start first.
    pub accounts: BrokerAccounts,
    /// The meter's source name, node label key and period.
    pub config: MeterConfig,
}

fn refused(setting: &str, reference: &SecretRef, why: &str) -> anyhow::Error {
    anyhow::Error::new(ReferenceRefused(format!("{setting} {reference:?}: {why}")))
}

/// The meter's parts, or `None` while `metering.enabled` is off (then no request is made and no file is read).
///
/// Must run inside a Tokio runtime: the sink starts its task.
///
/// # Errors
///
/// The bearer cannot be read or is blank, the CA bundle cannot be read, or a client cannot be built.
pub fn parts(metering: Option<&Metering>) -> Result<Option<Parts>> {
    let Some(m) = metering else {
        return Ok(None);
    };
    let reference = &m.broker_token;
    let text = match reference {
        SecretRef::File(path) => std::fs::read_to_string(path).map_err(|e| {
            refused(
                "metering.broker.token",
                reference,
                &format!("the file cannot be read ({})", e.kind()),
            )
        })?,
        SecretRef::Env(name) => std::env::var(name).map_err(|_| {
            refused(
                "metering.broker.token",
                reference,
                "the variable is not set",
            )
        })?,
    };
    let token = Token::new(&text)
        .map_err(|_| refused("metering.broker.token", reference, "the bearer is empty"))?;
    let accounts = BrokerAccounts::new(&m.broker_url, token, m.broker_timeout)?;
    let sink = OtlpUsageSink::start(m.collector.clone()).map_err(|_| {
        // `SettingsError` names a setting and never a value; the CA file is the one a file read can fail on.
        anyhow::Error::new(ReferenceRefused(
            "metering.collector: the sink could not start (is `caFile` readable and a PEM bundle?)"
                .to_owned(),
        ))
    })?;
    Ok(Some(Parts {
        sink: SharedSink(Arc::new(sink)),
        accounts,
        config: MeterConfig {
            source: m.source.clone(),
            node_type_label_key: Some(m.node_type_label_key.clone()),
            period: m.period,
            ..MeterConfig::default()
        },
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use aap_usage::MemoryAccounts;

    fn metering(token: &str) -> Metering {
        Metering {
            source: "m1".into(),
            node_type_label_key: "example.com/node-type".into(),
            period: Duration::from_secs(10),
            broker_url: url::Url::parse("http://broker.ns.svc:8080").unwrap(),
            broker_token: serde_yaml::from_str(token).unwrap(),
            broker_timeout: Duration::from_secs(5),
            collector: aap_usage_otlp::UsageOtlpSettings::new(
                url::Url::parse("http://collector.ns.svc:4318").unwrap(),
            ),
        }
    }

    #[tokio::test]
    async fn off_builds_nothing() {
        assert!(parts(None).unwrap().is_none());
    }

    #[tokio::test]
    async fn the_parts_carry_the_files_settings() {
        let dir = std::env::temp_dir().join(format!("aap-metering-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, "meter-token\n").unwrap();
        let m = metering(&format!("{{ file: {} }}", file.display()));
        let p = parts(Some(&m)).unwrap().unwrap();
        assert_eq!(p.config.source, "m1");
        assert_eq!(p.config.period, Duration::from_secs(10));
        assert_eq!(
            p.config.node_type_label_key.as_deref(),
            Some("example.com/node-type")
        );
        p.sink.0.shutdown(Duration::from_millis(100)).await;
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn an_unusable_bearer_is_a_refusal_that_names_the_reference_and_not_the_value() {
        let dir = std::env::temp_dir().join(format!("aap-metering-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty");
        std::fs::write(&empty, "  \n").unwrap();
        for reference in [
            format!("{{ file: {} }}", dir.join("missing").display()),
            format!("{{ file: {} }}", empty.display()),
            "{ env: AAP_TEST_NO_SUCH_METER_TOKEN }".to_owned(),
        ] {
            let m = metering(&reference);
            let Err(error) = parts(Some(&m)) else {
                panic!("{reference}: a meter with no bearer was built");
            };
            assert!(crate::config::is_reference_refused(&error), "{reference}");
            let text = format!("{error:#}");
            assert!(text.contains("metering.broker.token"), "{text}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_run_goes_to_the_run_source_and_an_agent_to_the_agent_source() {
        let run_source = MemoryAccounts::new();
        run_source.set_run("coder", "r1", "acct-run");
        let agent_source = MemoryAccounts::new();
        agent_source.set_agent("ns", "coder", "acct-agent");
        let accounts = MeterAccounts::new(run_source, agent_source);
        let run = PodRef::Run {
            service: "coder".into(),
            run: "r1".into(),
        };
        let agent = PodRef::Agent {
            namespace: "ns".into(),
            service: "coder".into(),
        };
        assert_eq!(
            accounts.account(&run).await,
            Ok(Some("acct-run".to_owned()))
        );
        assert_eq!(
            accounts.account(&agent).await,
            Ok(Some("acct-agent".to_owned()))
        );
        assert_eq!(accounts.run.asked(), vec![run]);
        assert_eq!(accounts.agent.asked(), vec![agent]);
    }
}
