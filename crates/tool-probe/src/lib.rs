//! The MCP probe of a `ToolProvider` (K3b, AD-053).
//!
//! The operator may ask a remote MCP server what tools it has, and write the reachability and the tool names into the
//! `ToolProvider`'s status. This crate is that request and nothing else: [`probe`] speaks MCP's **Streamable HTTP**
//! transport (a `POST` of `initialize`, the `notifications/initialized` notification, then `tools/list`, with the
//! `Mcp-Session-Id` and `MCP-Protocol-Version` headers the server asks for), accepts a JSON or an SSE answer, and returns a
//! [`ProbeOutcome`] with a closed reason and the tool names. It never sends a header of the object (AD-024: a server that
//! needs one is `NeedsHeader`, decided by the caller), never follows a redirect, uses no proxy, reads at most 1 MiB and
//! lists at most [`MAX_TOOLS`] names (the rest are counted).
//!
//! A probe is a client, not a provider seam: there is one transport and no backend to swap, so it is a function and not a
//! trait (AD-020 is about the implementations a binary composes). Its testkit is a stub MCP server,
//! [`testkit::StubMcp`](crate::testkit::StubMcp), which the controller's tests and the kind end-to-end use too.
//!
//! **The legacy HTTP+SSE transport (2024-11-05) is not spoken**: a server that only offers it answers a `POST` with a
//! `405`/`404`, and the outcome is [`ProbeReason::NotMcp`]. A server that requires a header of the object is
//! [`ProbeReason::NeedsHeader`], and one the deployment did not make reachable in its `NetworkPolicy` is
//! [`ProbeReason::Unreachable`] (the chart's `toolProbe`, K3a).
//!
//! No error, reason or log from this crate holds the URL, a header value or a response body: only a closed reason and
//! the tool names (which the server itself publishes) leave it.

#![warn(missing_docs)]

mod client;

pub use client::{MAX_TOOL_NAME, MAX_TOOLS, PROTOCOL_VERSION, ProbeOutcome, ProbeReason, probe};

#[cfg(feature = "testkit")]
pub mod testkit;
