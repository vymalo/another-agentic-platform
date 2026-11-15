//! The testkit (feature `testkit`): an exporter in memory, the conformance suite every `SpanExporter` must pass, and
//! `assert_clean`, which fails a test when a span holds what no span may hold.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::time::Instant;

use crate::attr::{Attr, Method, Value};
use crate::pipeline::{ExportError, Resource, SpanExporter};
use crate::span::{FinishedSpan, MAX_ATTRS, SpanKind, SpanName};

/// Spans kept in memory, in the order they ended.
#[derive(Clone, Debug, Default)]
pub struct Memory(Arc<Mutex<Vec<FinishedSpan>>>);

impl Memory {
    pub(crate) fn push(&self, span: FinishedSpan) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(span);
    }

    /// The spans so far.
    #[must_use]
    pub fn spans(&self) -> Vec<FinishedSpan> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The span named `name`, the first.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<FinishedSpan> {
        self.spans().into_iter().find(|s| s.name.as_str() == name)
    }
}

/// What a [`MemoryExporter`] does with a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Keep it.
    Keep,
    /// Fail it.
    Fail,
    /// Never answer.
    Hang,
}

/// A `SpanExporter` that keeps what it gets, and can be told to fail or to hang.
#[derive(Clone, Debug)]
pub struct MemoryExporter {
    kept: Memory,
    calls: Arc<Mutex<Vec<Instant>>>,
    mode: Arc<AtomicU8>,
    episodes: Arc<AtomicUsize>,
}

impl Default for MemoryExporter {
    fn default() -> Self {
        Self::new(Mode::Keep)
    }
}

impl MemoryExporter {
    /// An exporter in `mode`.
    #[must_use]
    pub fn new(mode: Mode) -> Self {
        let e = Self {
            kept: Memory::default(),
            calls: Arc::default(),
            mode: Arc::new(AtomicU8::new(0)),
            episodes: Arc::default(),
        };
        e.set_mode(mode);
        e
    }

    /// Change the mode.
    pub fn set_mode(&self, mode: Mode) {
        self.mode.store(
            match mode {
                Mode::Keep => 0,
                Mode::Fail => 1,
                Mode::Hang => 2,
            },
            Ordering::SeqCst,
        );
    }

    /// The spans kept.
    #[must_use]
    pub fn spans(&self) -> Vec<FinishedSpan> {
        self.kept.spans()
    }

    /// How many times the pipeline said that exports started failing ([`SpanExporter::export_failing`]).
    #[must_use]
    pub fn failing_episodes(&self) -> usize {
        self.episodes.load(Ordering::SeqCst)
    }

    /// When each export was asked for.
    #[must_use]
    pub fn call_times(&self) -> Vec<Instant> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl SpanExporter for MemoryExporter {
    async fn export(
        &self,
        _resource: &Resource,
        batch: Vec<FinishedSpan>,
    ) -> Result<(), ExportError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Instant::now());
        match self.mode.load(Ordering::SeqCst) {
            0 => {
                for s in batch {
                    self.kept.push(s);
                }
                Ok(())
            }
            1 => Err(ExportError::Failed),
            _ => std::future::pending().await,
        }
    }

    fn export_failing(&self) {
        self.episodes.fetch_add(1, Ordering::SeqCst);
    }
}

/// `n` made-up spans, with ids that tell them apart and one attribute each.
#[must_use]
pub fn sample_spans(n: usize) -> Vec<FinishedSpan> {
    (0..n)
        .map(|i| {
            let mut id = [0u8; 8];
            id.copy_from_slice(&(i as u64 + 1).to_be_bytes());
            FinishedSpan {
                trace_id: [7; 16],
                span_id: id,
                parent_id: None,
                name: SpanName::method(Method::Get),
                kind: SpanKind::Internal,
                start_unix_nanos: 1_000 + i as u64,
                end_unix_nanos: 2_000 + i as u64,
                attrs: vec![Attr::HttpMethod(Method::Get)],
                error: false,
            }
        })
        .collect()
}

/// What an implementation gives [`exporter_conformance!`](crate::exporter_conformance).
pub struct Harness<E> {
    /// The exporter, ready to deliver.
    pub exporter: E,
    /// The span ids of everything it delivered so far, in order, as the other end saw them.
    pub delivered: Box<dyn Fn() -> Vec<[u8; 8]> + Send + Sync>,
    /// Make its next exports fail (the collector is down).
    pub fail: Box<dyn Fn() + Send + Sync>,
}

/// The conformance suite of a [`SpanExporter`](crate::SpanExporter): `exporter_conformance!(make)` where `make` is an
/// `async fn() -> Harness<E>`. The crate that uses it needs `tokio` with `macros` and `rt` as a dev-dependency.
///
/// * a batch is delivered once, with its spans in order and every attribute;
/// * a failure is an `Err`, not a panic, and nothing is delivered by it;
/// * the pipeline's flush delivers what was queued.
#[macro_export]
macro_rules! exporter_conformance {
    ($make:path) => {
        #[tokio::test]
        async fn exporter_delivers_once_and_in_order() {
            let h = $make().await;
            let resource = $crate::Resource::new("conformance", "0");
            let first = $crate::testkit::sample_spans(3);
            let mut second = $crate::testkit::sample_spans(5);
            second.drain(..3);
            $crate::SpanExporter::export(&h.exporter, &resource, first.clone())
                .await
                .unwrap();
            $crate::SpanExporter::export(&h.exporter, &resource, second.clone())
                .await
                .unwrap();
            let want: Vec<[u8; 8]> = first.into_iter().chain(second).map(|s| s.span_id).collect();
            assert_eq!((h.delivered)(), want);
        }

        #[tokio::test]
        async fn exporter_failure_is_an_err_and_delivers_nothing() {
            let h = $make().await;
            let resource = $crate::Resource::new("conformance", "0");
            (h.fail)();
            let r = $crate::SpanExporter::export(
                &h.exporter,
                &resource,
                $crate::testkit::sample_spans(2),
            )
            .await;
            assert!(r.is_err());
            assert!((h.delivered)().is_empty());
        }

        #[tokio::test]
        async fn exporter_gets_everything_the_flush_hands_it() {
            let h = $make().await;
            let delivered = h.delivered;
            let tracer = $crate::Pipeline::start(
                h.exporter,
                $crate::Limits::default(),
                $crate::Resource::new("conformance", "0"),
                $crate::Sampler::AlwaysOn,
            );
            for _ in 0..10 {
                tracer
                    .root(
                        $crate::SpanName::method($crate::Method::Get),
                        $crate::SpanKind::Internal,
                    )
                    .end();
            }
            tracer.shutdown(::std::time::Duration::from_secs(5)).await;
            assert_eq!((delivered)().len(), 10);
        }
    };
}

/// Whether `s` looks like a credential: the patterns of `render-check.sh`.
#[must_use]
pub fn looks_like_secret(s: &str) -> bool {
    let after =
        |p: &str, f: &dyn Fn(&str) -> bool| s.match_indices(p).any(|(i, _)| f(&s[i + p.len()..]));
    let run = |rest: &str, n: usize, ok: &dyn Fn(char) -> bool| {
        rest.chars().take(n).filter(|c| ok(*c)).count() == n
    };
    s.contains("ghp_")
        || s.contains("github_pat_")
        || s.contains("gho_")
        || s.contains("-----BEGIN")
        || s.contains("xoxb-")
        || s.contains("xoxp-")
        || after("sk-", &|r| run(r, 8, &|c| c.is_ascii_alphanumeric()))
        || after("AKIA", &|r| {
            run(r, 16, &|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        || after("eyJ", &|r| {
            run(r, 20, &|c| {
                c.is_ascii_alphanumeric() || c == '_' || c == '-'
            })
        })
}

/// Fail the test when a span holds what no span may hold: a key outside [`Attr::ALL_KEYS`], more than 16 attributes,
/// a key twice, a value or a name that contains one of `sentinels` or looks like a credential.
///
/// # Panics
///
/// With the span's name and the key, never the value.
pub fn assert_clean(spans: &[FinishedSpan], sentinels: &[&str]) {
    let dirty =
        |s: &str| sentinels.iter().any(|x| !x.is_empty() && s.contains(x)) || looks_like_secret(s);
    for span in spans {
        assert!(
            !dirty(span.name.as_str()),
            "span name of {:?} is not clean",
            span.kind
        );
        assert!(
            span.attrs.len() <= MAX_ATTRS,
            "{}: too many attributes",
            span.name.as_str()
        );
        let mut seen = std::collections::HashSet::new();
        for a in &span.attrs {
            assert!(
                Attr::ALL_KEYS.contains(&a.key()),
                "{}: key outside the list",
                span.name.as_str()
            );
            assert!(
                seen.insert(a.key()),
                "{}: {} twice",
                span.name.as_str(),
                a.key()
            );
            if let Value::Str(v) = a.value() {
                assert!(
                    !dirty(v),
                    "{}: {} is not clean",
                    span.name.as_str(),
                    a.key()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_secret_patterns_match_render_check() {
        for bad in [
            "ghp_x",
            "github_pat_x",
            "gho_x",
            "sk-abcdefgh1",
            "-----BEGIN KEY",
            "AKIAABCDEFGHIJKLMNOP",
            "xoxb-1",
            "xoxp-1",
            "eyJabcdefghijklmnopqrstu",
        ] {
            assert!(looks_like_secret(bad), "{bad}");
        }
        for good in [
            "sk-short",
            "AKIAshort",
            "eyJshort",
            "GET /v1/agents",
            "reconcile agentservice",
        ] {
            assert!(!looks_like_secret(good), "{good}");
        }
    }

    #[test]
    #[should_panic(expected = "is not clean")]
    fn assert_clean_catches_a_sentinel() {
        let mut spans = sample_spans(1);
        spans[0]
            .attrs
            .push(Attr::error_type("sentinel-value").unwrap());
        assert_clean(&spans, &["sentinel"]);
    }

    #[test]
    #[should_panic(expected = "twice")]
    fn assert_clean_catches_a_key_twice() {
        let mut spans = sample_spans(1);
        spans[0].attrs.push(Attr::HttpMethod(Method::Put));
        assert_clean(&spans, &[]);
    }

    #[test]
    #[should_panic(expected = "too many")]
    fn assert_clean_catches_too_many_attributes() {
        let mut spans = sample_spans(1);
        spans[0].attrs = (0..17).map(|_| Attr::HttpStatus(200)).collect();
        assert_clean(&spans, &[]);
    }

    #[test]
    fn a_clean_span_passes() {
        assert_clean(&sample_spans(3), &["sentinel"]);
    }
}
