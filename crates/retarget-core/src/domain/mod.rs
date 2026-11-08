//! The rules and types of the problem, in its own words.
//!
//! The aim is the performance of a customer's agent or swarm: what a
//! successful task costs, how long it takes, how often it succeeds. Swarms
//! waste in recurring ways (a subagent's cache that expires and is written
//! again, the same context handed to several subagents, reasoning spent on a
//! small task), and each way has fixes. Statistics, rules, machine learning and
//! AI are tools toward that aim; which one is used is decided by what it does
//! for the performance, not by how rigorous it is.
//!
//! A **lever** is one thing about how the agent runs that can be changed: a
//! setting of the harness, how context is handled, how deeply the model
//! thinks, which model a role gets, what an instruction says. A **technique**
//! is a known fix: when a **pattern** shows in a customer's runs, move a lever
//! this way. A **catalogue** holds techniques with what they are expected to do
//! and how to tell whether they fit. The **ledger** of a scope holds what was
//! applied, what was put back and why.
//!
//! # Two layers
//!
//! ```text
//! Lab (in house)                                  product (at the customer)
//!   explore, and prove with robust statistics       runs ──> find a pattern
//!   which techniques work, where, by how much       ──> apply its technique (shown, some runs, all)
//!         │                                         ──> check the fit against the catalogue's limits
//!         └── catalogue (a versioned value) ───────>    ──> keep | put back, try the next | keep watching
//! ```
//!
//! - **The product** applies what the catalogue knows and adapts it to one
//!   customer: it finds a pattern, applies the technique in a way that can be
//!   put back, and checks that it fits with the limits the catalogue fixed for
//!   it (a holdout of runs, a failure limit, a window), without proving the
//!   technique again from scratch. What it expects and what it sees so far are
//!   always shown; nothing waits silently for a verdict.
//! - **The Lab** is where techniques are found and proven: experiments with
//!   random assignment and statistics that stay valid under repeated looks
//!   (see [The Lab](#the-lab-how-a-technique-is-proven)). Proving a small
//!   effect takes months of runs, which is fine in house and not in front of a
//!   customer.
//! - The framework is open: the core, the adapters, the Lab's machinery and a
//!   catalogue built from public facts. Data, a tuned catalogue and the
//!   features that find patterns are not part of this repository; they plug in
//!   as a catalogue value and as implementations of a port.
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
//! - **What a decision rests on**: the objective, the guardrails, the kind of
//!   evidence a lever needs, and when to stop.
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
//! # The Lab: how a technique is proven
//!
//! An **experiment** moves one lever by one step and reaches a **verdict**
//! ([`lab::experiment`], [`lab::verdict`], [`stats`]). The objective is the cost per
//! successful run, within one task class. It is the number that matters and
//! the slowest to settle: a run's cost varies widely from task to task, so a
//! fixed before-and-after comparison of whole runs needs far more runs than
//! one user produces before it can tell a small improvement from noise. This
//! is why proving belongs to the Lab, and why the product applies what was
//! proven instead. In the Lab, a lever is judged on the evidence its kind
//! allows:
//!
//! - A lever that only changes what the same work costs (a cache lifetime) is
//!   judged on the cost terms its mechanism moves, per unit of the terms the
//!   step leaves alone, so that the size of a run does not drown the change.
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
//! shared   name, money ──> run ──> cost ──> mechanism ──> lever ──> assign
//!          stats ──> measure (which runs count, the gate, guardrails, estimators)
//!          put_back ──> places (the guard on writing a lever's place)
//! lab      experiment ──> verdict ──> ledger                 (uses shared only)
//! product  catalogue ──> finding ──> adoption ──> fit ──> portfolio   (uses shared only)
//! ```

pub mod assign;
pub mod cost;
pub mod lab;
pub mod lever;
pub mod measure;
pub mod mechanism;
pub mod money;
pub mod name;
pub mod places;
pub mod product;
pub mod put_back;
pub mod run;
pub mod stats;

#[cfg(test)]
mod testkit;
