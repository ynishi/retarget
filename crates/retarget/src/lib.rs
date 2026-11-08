//! Tune a coding agent's harness one setting at a time, and keep only what
//! measurably helps.
//!
//! retarget watches the telemetry a coding agent's harness already sends,
//! changes one thing about how the agent runs, compares runs with and without
//! the change, and keeps it only when the evidence clears a rule fixed in
//! advance. Otherwise it puts the change back. The measure that decides is the
//! cost per successful run; how a verdict is reached is in
//! [`domain`]. It adds no proxy and no memory layer between the agent and its
//! model.
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
//! - **lever**: one thing about how the agent runs that retarget may change (a
//!   setting, how context is handled, reasoning depth, the model of a role, an
//!   instruction).
//! - **experiment**: one step of one lever, with its hypothesis, threshold and
//!   stopping rule fixed before it starts. Nothing is written or put back over
//!   a person's edit; the experiment then stops and says so.
//! - **run**: one session of the agent, taken as one task, observed as a tree
//!   of agent nodes and their requests, tool calls and incidents.
//! - **arm**: the position of the lever a run is assigned to when it starts.
//! - **cost term**: one part of a run's cost (cache writes, cache reads,
//!   uncached input, output, rework, off-main work).
//! - **mechanism**: a way a lever moves cost terms, independent of any harness.
//! - **guardrail**: a measure that must not get worse (success rate, duration,
//!   rework), or a trust check that must hold for a verdict to mean anything.
//! - **holdout**: a small share of runs left on the old position, against which
//!   a kept change goes on being measured.
//! - **verdict**: pending, provisionally kept, kept, reverted (with its reason),
//!   tripped (too many failures), or halted (a trust check broke, or someone
//!   edited the lever's file; the change is put back without a verdict when
//!   that is safe).
//! - **ledger**: the experiments of one scope, what it has given up on and when
//!   it may try again, and how well each mechanism's terms have foretold the
//!   objective.
//! - **task class**: the group a run is compared within (repository,
//!   language, size).
//! - **off-main request**: a model request outside the main conversation
//!   (subagents, compaction, background work).
//!
//! # Map
//!
//! - [`domain`]: runs, cost terms, mechanisms, levers, experiments, verdicts,
//!   the ledger.
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
//! machine, and is trusted as far as it is well-formed. retarget writes only
//! files of the harness's project scope that it can put back, records what they
//! held first, and does not write or put back a file someone has edited since
//! (the rule is stated in [`domain::put_back`]). It never writes the user's own settings,
//! permissions or credentials, and shows those to the user instead.
//!
//! # Compatibility
//!
//! Before 1.0, any release may change the library API, the command line and the
//! stored events.
#![warn(missing_docs)]

pub use retarget_core::*;

pub mod infra;
