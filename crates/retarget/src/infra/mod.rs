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
//! definitions, its project instructions), and records what a file held before
//! writing it. Before putting a file back, the adapter checks that it still
//! holds what retarget wrote; a file someone has edited since is not put back,
//! and the experiment stops and says so. Nothing in the user's own scope is
//! written: a setting there (the telemetry environment, the status line) is
//! shown to the user, who sets it.
