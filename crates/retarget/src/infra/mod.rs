//! Adapters: the ports of `retarget_core::app` implemented with real I/O (the
//! telemetry the harness sends, the event store, the harness's settings files,
//! git).
//!
//! The outside world's formats and errors are translated here, at the edge,
//! so that nothing past this layer sees them: telemetry attributes become a
//! session's usage, a settings file becomes a lever's state, the store's event
//! envelope becomes a domain event. An adapter that grows large moves to a
//! crate of its own.
//!
//! What may be written is bounded: an adapter writes the project-scoped
//! settings of the harness and nothing in the user's own scope. A setting of
//! the user's scope (the telemetry environment, the status line) is shown to the
//! user, who sets it.
