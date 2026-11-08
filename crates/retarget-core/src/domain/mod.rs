//! The rules and types of the problem, in its own words.
//!
//! A **lever** is one thing about how the agent runs that can be changed: a
//! setting of the harness, how context is handled, how deeply the model
//! thinks, which model a role gets, what an instruction says. An
//! **experiment** moves one lever by one step and reaches a **verdict**. The
//! **ledger** of a scope holds its experiments and what it has given up on,
//! and allows one active experiment at a time.
//!
//! # What is fixed here, and what an adapter fills in
//!
//! This module fixes three things and no harness vocabulary:
//!
//! - **Shape.** A session is observed as a **run**: a tree of agent nodes
//!   (the main conversation, each subagent, auxiliary work such as
//!   compaction), each holding its model requests, tool calls and incidents
//!   (a retry, an error, a compaction). The kinds of element are few and
//!   closed; the configuration of agents is not, so it is a value, one tree
//!   per run.
//! - **Meaning.** The cost of a successful run is split into **cost terms**
//!   (cache writes, cache reads, uncached input, output, rework, off-main
//!   work), and a **mechanism** is a way a lever moves those terms (a cache
//!   that expires and is written again, context that grows, the same context
//!   handed to several subagents, the volume of reasoning, the price of a
//!   model, rework after a failure). Which terms a mechanism lowers, and which
//!   it may raise, is knowledge of this domain, not of any harness.
//! - **The specification of a verdict**: the objective, the guardrails, the
//!   kind of evidence a lever needs, and the stopping rule.
//!
//! What a harness calls an attribute, which event carries the parent of a
//! subagent, what a token costs: those are how a measure is taken, and they
//! belong to the adapters. This is the split between what a service level
//! indicator specifies and how it is implemented
//! (<https://sre.google/workbook/implementing-slos/>). A lever arrives from an
//! adapter's catalogue as a value that declares its shape: its positions, the
//! surface it is written to, the mechanisms it claims to act on, the role it
//! is scoped to. Adding a lever changes the catalogue, not this module.
//!
//! # How a verdict is reached
//!
//! The objective is the cost per successful run, within one task class. It is
//! the number that matters and the slowest to settle: a run's cost varies
//! widely from task to task, so a fixed before-and-after comparison of whole
//! runs needs far more runs than one user produces before it can tell a small
//! improvement from noise. Judging on it alone leaves nearly every experiment
//! pending. Judging only what can be measured exactly leaves most levers out.
//! So a lever is judged on the evidence its kind allows:
//!
//! - A lever that only changes what the same work costs (a cache lifetime) is
//!   judged on the cost terms its mechanism moves, measured on the requests of
//!   every run, which vary far less from run to run than the whole cost does.
//!   Which mechanisms only change prices is a table of this domain
//!   ([`mechanism::evidence_of`]). It is kept **provisionally**, and
//!   only when the terms it should lower fall past the threshold and the terms
//!   it may raise do not eat the saving: a token saving that the bill does not
//!   show is not kept (arXiv 2607.12161 v5: the largest compression cut tool
//!   output by 38.4% and raised the bill by 6.8%).
//! - A lever that changes how the agent works (reasoning depth, how context is
//!   handled, the model of a role) is judged on the objective directly.
//!
//! Either way:
//!
//! - The threshold and the stopping rule are fixed before the change and do
//!   not move after it. The stopping rule stays valid however often it is
//!   looked at, because a test watched every day and stopped when it looks
//!   good is not a test (Johari, Pekelis and Walsh, "Always Valid Inference",
//!   arXiv 1512.04922). It also bounds the number of runs: an experiment that
//!   reaches it undecided is reverted, not left pending.
//! - Runs are assigned to an arm at random when they start, so time and
//!   harness upgrades do not decide which arm looks better. A run that spans
//!   a change counts for neither arm.
//! - The guardrails (success rate, duration, rework) must not get worse beyond
//!   a margin fixed in advance. Failures past a limit stop the experiment
//!   without waiting for successes.
//! - A broken trust check stops the experiment without a verdict: arms out of
//!   balance, a change that does not take effect (something of higher
//!   precedence overrides it), costs taken from different sources in the two
//!   arms.
//! - A provisional keep is confirmed or reverted on the objective, against a
//!   small **holdout** of runs left on the old position, by a margin fixed
//!   with the experiment, which may be smaller than the threshold the cost
//!   terms had to clear, since those terms are only part of the bill. The holdout stays
//!   after the lever is kept, so a saving that a later upgrade erases is
//!   noticed.
//! - The ledger records, for each mechanism, how often its cost terms foretold
//!   the objective. A mechanism whose terms keep failing to is judged on the
//!   objective from then on. A short-term measure stands in for the long-term
//!   one only under an assumption that has no testable implication of its own,
//!   so the only check is the long-term outcome itself (Athey, Chetty, Imbens
//!   and Kang, "The Surrogate Index", NBER working paper 26463).
//!
//! What was tried and put back is kept with the condition under which it may
//! be tried again, so the same change is not tried twice for nothing.
//!
//! ```text
//! run tree ──> cost terms per run ──┬──> objective (cost per successful run)
//!                                   └──> terms of the lever's mechanism
//! lever ──> mechanisms ──> hypothesis: these terms fall, those may rise
//!
//! prices only:     terms fall, saving not eaten ──> provisional keep
//!                  objective against the holdout ──> keep | revert
//! behaviour:       objective ──> keep | revert
//! both:            guardrails not worse, trust checks hold, stopping rule
//! ```
//!
//! # Purity
//!
//! Nothing here touches the file system, a process, the environment, the
//! network, the clock or a random source. A value that comes from outside (a
//! run's tree, the current time, a new id, the draw that picks an arm) is
//! passed in by the caller. A value with rules is a type, not a `String`: an
//! enum for a closed set, a newtype that checks its input for anything else.
//!
//! # Map
//!
//! ```text
//! name, money ──> run ──> cost ──> mechanism ──> lever ──> experiment ──> verdict ──> ledger
//!                 stats (leaf)                     put_back (uses name only) ──────────┘
//! ```

pub mod cost;
pub mod experiment;
pub mod ledger;
pub mod lever;
pub mod mechanism;
pub mod money;
pub mod name;
pub mod put_back;
pub mod run;
pub mod stats;
pub mod verdict;

#[cfg(test)]
mod testkit;
