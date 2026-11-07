//! Core (SDK) of retarget: the rules that decide whether a change to a coding
//! agent's harness is kept, and the use cases that apply them.
//!
//! Two layers, inner to outer. `arch-lint.toml` at the workspace root states
//! them, and `cargo test` (`tests/architecture.rs`) fails when code breaks them.
//!
//! - [`domain`]: the rules and types of the problem. Pure: no file system,
//!   process, environment, network, clock or randomness. Those come in as values.
//! - [`app`]: the use cases, and the ports (traits) they need from the outside
//!   world. Also free of direct I/O.
//!
//! The adapters that implement the ports with real I/O (the telemetry receiver,
//! the event store, the harness's settings files) live in the umbrella crate
//! `retarget`, not here. This crate has no I/O dependency at all, so that a
//! change of store or of harness never changes it; `tests/dependencies.rs`
//! fails when one is added.
#![warn(missing_docs)]

pub mod app;
pub mod domain;
