//! The rules and types of the problem, in its own words.
//!
//! A **lever** is one setting of the harness that can be changed. An
//! **experiment** changes one lever, measures sessions before and after, and
//! reaches a **verdict**: keep the change, revert it, wait for more sessions, or
//! report that the change is not taking effect. The **ledger** of a scope holds
//! its experiments and the levers it has given up on, and allows one active
//! experiment at a time.
//!
//! The main measure is the cost per successful session, compared within one
//! task class. Token counts and cache rates explain a verdict; they do not make
//! it, because a token reduction is not a cost reduction (arXiv 2607.12161 v5:
//! the largest compression cut tool output by 38.4% and raised the bill by 6.8%).
//!
//! Pure: nothing here touches the file system, a process, the environment, the
//! network, the clock or a random source. A value that comes from outside
//! (a session's usage, the current time, a new id) is passed in by the caller.
//! A value with rules is a type, not a `String`: an enum for a closed set,
//! a newtype that checks its input for anything else.
