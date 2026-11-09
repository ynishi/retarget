//! Adapters: the ports of `retarget_core::app` implemented with real I/O (the
//! telemetry the harness sends, the event store, the harness's files, git).
//!
//! The outside world's formats and errors are translated here, at the edge,
//! so that nothing past this layer sees them:
//!
//! - Telemetry becomes a run's tree. Which event or attribute names the parent
//!   of a subagent, which values one attribute takes in a metric and another
//!   in an event, what changed between two versions of the harness: all of it
//!   is known here and nowhere else. A value the harness did not send is
//!   passed on as missing, never guessed.
//! - A cost the harness reports is used as it is. When it reports none, it is
//!   priced here from a table of prices, and the run says which of the two it
//!   carries.
//! - A harness's catalogue of levers (what can be changed, its positions,
//!   where it is written, the mechanisms it acts on) is kept here, per harness
//!   and version.
//! - A file of the harness becomes a lever's position, and the store's event
//!   envelope becomes a domain event.
//!
//! An adapter that grows large moves to a crate of its own.
//!
//! What may be written is bounded: an adapter writes only files of the
//! harness's project scope that can be put back (its settings, its agent
//! definitions, its project instructions). Before writing a lever's place in a
//! file, or putting it back, the adapter reads what that place holds now (the
//! value at the lever's key, or the section it owns, not the whole file) and
//! hands its digest to the core, which decides whether the write may go ahead
//! ([`retarget_core::domain::put_back`]). Nothing in the user's own scope is
//! written: a setting there (the telemetry environment, the status line) is
//! shown to the user, who sets it.
//!
//! # Adapters
//!
//! - [`claude_code`]: Claude Code's telemetry events, translated into runs.
//! - [`otlp`]: the OpenTelemetry encoding of logs, shared by any harness that
//!   sends it.
//! - [`price`]: prices per model, which the caller passes in, to price a
//!   request's terms when the harness reports only a total.

pub mod claude_code;
pub mod otlp;
pub mod price;
