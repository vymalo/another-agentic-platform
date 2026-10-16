//! A client that lets somebody else get in first: it forwards every request to a real client, and just before it
//! forwards the next real write of an object it runs a hook (with another client, an administrator's), so that the
//! window between the dry runs and the applies of the crate is a race that a real API server decides.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use http::{Request, Response};
use kube::Client;
use kube::client::Body;
use tower::Service;

type Hook = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

struct Pending {
    method: &'static str,
    plural: String,
    name: String,
    run: Hook,
}

/// Where hooks are put.
#[derive(Clone, Default)]
pub struct Hooks {
    pending: Arc<Mutex<Vec<Pending>>>,
}

impl Hooks {
    /// Run `run` once, just before the next server-side apply of `plural/name` that is not a dry run.
    pub fn before_the_next_apply<F, Fut>(&self, plural: &str, name: &str, run: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.push("PATCH", plural, name, run);
    }

    /// Run `run` once, just before the next delete of `plural/name` that is not a dry run.
    pub fn before_the_next_delete<F, Fut>(&self, plural: &str, name: &str, run: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.push("DELETE", plural, name, run);
    }

    fn push<F, Fut>(&self, method: &'static str, plural: &str, name: &str, run: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Pending {
                method,
                plural: plural.to_owned(),
                name: name.to_owned(),
                run: Box::new(move || Box::pin(run())),
            });
    }

    fn take(&self, method: &str, path: &str, dry_run: bool) -> Option<Hook> {
        if dry_run {
            return None;
        }
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let at = pending.iter().position(|p| {
            p.method == method && path.ends_with(&format!("/{}/{}", p.plural, p.name))
        })?;
        Some(pending.remove(at).run)
    }
}

#[derive(Clone)]
struct Interpose {
    inner: Client,
    hooks: Hooks,
}

impl Service<Request<Body>> for Interpose {
    type Response = Response<Body>;
    type Error = tower::BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, tower::BoxError>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let (inner, hooks) = (self.inner.clone(), self.hooks.clone());
        Box::pin(async move {
            let (parts, body) = req.into_parts();
            let bytes = body.collect_bytes().await.map_err(tower::BoxError::from)?;
            let dry_run = match parts.method.as_str() {
                // A delete takes its options from its body when it has one.
                "DELETE" if !bytes.is_empty() => {
                    serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .and_then(|b| b.get("dryRun").cloned())
                        .is_some_and(|d| d.as_array().is_some_and(|a| !a.is_empty()))
                }
                _ => parts.uri.query().is_some_and(|q| q.contains("dryRun")),
            };
            if let Some(hook) = hooks.take(parts.method.as_str(), parts.uri.path(), dry_run) {
                hook().await;
            }
            inner
                .send(Request::from_parts(parts, Body::from(bytes.to_vec())))
                .await
                .map_err(tower::BoxError::from)
        })
    }
}

/// `client`, with hooks.
pub fn interpose(client: &Client, namespace: &str) -> (Client, Hooks) {
    let hooks = Hooks::default();
    let service = Interpose {
        inner: client.clone(),
        hooks: hooks.clone(),
    };
    (Client::new(service, namespace), hooks)
}
