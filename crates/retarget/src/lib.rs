//! Tune a coding agent's harness one setting at a time, and keep only what
//! measurably helps.
//!
//! retarget watches the telemetry a coding agent's harness already sends,
//! changes one of the harness's own settings, measures the sessions before and
//! after, and keeps the change only when the cost per successful session fell
//! past a threshold fixed in advance. Otherwise it puts the setting back. It
//! adds no proxy and no memory layer between the agent and its model.
//!
//! The umbrella crate: the one name users depend on, and the
//! `retarget` command (`src/main.rs`, behind the default `cli`
//! feature). It re-exports `retarget-core` and holds the adapters, the code
//! with real I/O. A library user writes `default-features = false` to leave out
//! what only the command needs. Its doc is the front page of the project's
//! documentation.
//!
//! # Glossary
//!
//! - **lever**: one setting of the harness that retarget may change.
//! - **experiment**: one change of one lever, with its threshold, measured
//!   before and after.
//! - **verdict**: keep, revert, pending (not enough sessions yet), or not
//!   taking effect (something of higher precedence overrides the setting).
//! - **ledger**: the experiments of one scope and the levers it has given up on.
//! - **session**: one run of the agent, taken as one task.
//! - **task class**: the group a session is compared within (repository,
//!   language, size).
//! - **off-main request**: a model request outside the main conversation
//!   (subagents, compaction, background work).
//!
//! # Map
//!
//! - [`domain`]: levers, experiments, verdicts, the ledger.
//! - [`app`]: the use cases and their ports.
//! - [`infra`]: the adapters.
//!
//! # Layers
//!
//! ```text
//! retarget (lib, adapters, and the command with `cli`) ──> retarget-core
//!
//! retarget:        infra ──> retarget_core::app
//! retarget-core:   app ──> domain
//! ```
//!
//! The core has no I/O dependency, so a change of event store, telemetry format
//! or harness changes only this crate.
//!
//! # Security
//!
//! The telemetry retarget reads is sent by the user's own harness on the same
//! machine, and is trusted as far as it is well-formed. retarget writes only the
//! project-scoped settings of the harness; it never writes the user's own
//! settings, permissions or credentials, and shows those to the user instead.
//!
//! # Compatibility
//!
//! Before 1.0, any release may change the library API, the command line and the
//! stored events.
#![warn(missing_docs)]

pub use retarget_core::*;

pub mod infra;
