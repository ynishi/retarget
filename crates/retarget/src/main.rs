//! The `retarget` command: parses arguments, calls the library,
//! prints the result. It is built only with the `cli` feature (on by default).
//!
//! The argument parser's definition (clap's derive types, once added, as an
//! optional dependency of the `cli` feature) is the only description of the
//! commands: help, the command list and validation come from it, not from a
//! second hand-kept table. Structured input is read from a file argument, not
//! pasted into the command line.
//!
//! How the command is used (the commands, the output, the exit codes) is the
//! "Command line" section of the crate doc in `src/lib.rs`. A long-running
//! process of the same user (a daemon, a watcher) is a subcommand of this
//! command. A server that someone hosts for others is a package of its own,
//! `retarget-server`, with its own binary, added when it is needed.
//!
//! A library with no command deletes this file, the `[[bin]]` and the `cli`
//! feature, and the "Command line" section.

fn main() {}
