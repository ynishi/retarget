//! The use cases: read what they need, decide with [`crate::domain`], record
//! the outcome.
//!
//! What a use case needs from the outside world is a port: a trait declared
//! here, by its consumer, and implemented by the adapters of the umbrella crate
//! `retarget`. Code here calls ports, never the file system, a process or the
//! environment directly.
//!
//! A decision on the ledger is taken inside the write: the use case reads the
//! ledger's events, the domain decides, and the new events are appended on the
//! condition that nothing was appended in between. When something was, the use
//! case reads again and decides again. This is what keeps one active experiment
//! per scope when two commands race.
//!
//! # Flow
//!
//! ```text
//! write:  command ──> read ledger events ──> domain decides ──> append (if unchanged)
//!                          ^                                        │
//!                          └──────────── changed in between ────────┘
//! read:   projection of the events ──> status line, notice
//! ```
