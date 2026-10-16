//! One pass, bounded and visible.
//!
//! kube's `Controller` runs one pass per object at a time and keeps the object's slot until the pass returns, so
//! that a pass that never returns takes the object out of the controller for good: the next trigger, the timer
//! of the last pass and the error policy all wait for it. A request to an API server has no timeout of its own
//! here (kube-client 4.0 sets none for a response: `Config::read_timeout` is `None`, because a watch, an `exec` or
//! a log stream is a response that does not end), and a call can be queued behind a stream on a pooled
//! connection (*seen 2026-10-16* on kind v1.35.8 with hyper 1.11.1: the README says how). So every pass gets a
//! limit, and is logged when it starts, when it ends and when it was slow.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use crate::error::Error;
use crate::metrics::{Metrics, name};

/// A pass that took this long is worth a warning even when it succeeded.
const SLOW_PASS: Duration = Duration::from_secs(10);

/// Run `pass` for at most `limit`. `controller` and `key` (`namespace/name`) say whose pass it is.
///
/// At `debug`: the start and the end of every pass, with its duration. At `warn`: a pass that took
/// [`SLOW_PASS`] or more, and (by the error policy, from the error this returns) one that was cut short.
pub(crate) async fn bounded<T>(
    controller: &'static str,
    key: &str,
    limit: Duration,
    metrics: &Metrics,
    pass: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let started = Instant::now();
    tracing::debug!(controller, object = key, "pass started");
    let result = match tokio::time::timeout(limit, pass).await {
        Ok(result) => result,
        Err(_elapsed) => {
            metrics.inc(name::PASS_TIMEOUTS, &[("controller", controller)]);
            Err(Error::TimedOut(limit))
        }
    };
    let elapsed = started.elapsed();
    if elapsed >= SLOW_PASS {
        tracing::warn!(
            controller,
            object = key,
            ?elapsed,
            ok = result.is_ok(),
            "slow pass"
        );
    } else {
        tracing::debug!(
            controller,
            object = key,
            ?elapsed,
            ok = result.is_ok(),
            "pass finished"
        );
    }
    result
}
