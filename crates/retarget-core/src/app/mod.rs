//! The use cases: read what they need, decide with [`crate::domain`], record
//! the outcome.
//!
//! What a use case needs from the outside world is a port: a trait declared
//! here, by its consumer, and implemented by the adapters of the umbrella crate
//! `retarget`. Code here calls ports, never the file system, a process or the
//! environment directly. The clock, new ids and the random draw that assigns a
//! run to an arm come through ports too.
//!
//! A decision on the ledger is taken inside the write: the use case reads the
//! ledger's events, the domain decides, and the new events are appended on the
//! condition that nothing was appended in between. When something was, the use
//! case reads again and decides again. This is what keeps one active experiment
//! per scope when two commands race.
//!
//! Writing to the harness is not part of that condition, so a decision and its
//! effect are separate events: the decision is appended first, the file is
//! written, and only then is the change recorded as applied or put back. A
//! write that fails leaves a decision without an effect, which the next command
//! sees and finishes, rather than a ledger that claims a change the files do
//! not have.
//!
//! The parts that need judgement no rule can give (which class a task is in,
//! whether it succeeded, what to try next, how good an output is) are ports
//! as well. A verdict never goes through them: they may propose and describe,
//! and the rules of the domain decide. What their answers cost is counted
//! against the objective, as retarget's own overhead.
//!
//! # Flow
//!
//! ```text
//! write:  command ──> read ledger events ──> domain decides ──> append (if unchanged)
//!                          ^                                        │
//!                          └──────────── changed in between ────────┘
//! effect: decision appended ──> write the harness's file ──> record applied / put back
//! run:    run starts ──> assign an arm ──> write its position
//!         run ends ──> its tree is recorded ──> evaluate the active experiment
//! read:   projection of the events ──> status line, notice
//! ```
//!
//! # Ports of the product
//!
//! What finds a pattern and where a catalogue comes from are ports, so that
//! tuned detectors and catalogues plug in from outside the repository. The
//! public ones are [`PublicDetector`] and a catalogue an adapter reads from a
//! file.

use crate::domain::product::catalogue::Catalogue;
use crate::domain::product::finding::{Finding, public_findings};
use crate::domain::run::Run;

/// Finds patterns of waste in recent runs.
pub trait Detector {
    /// The patterns found in these runs.
    fn detect(&self, runs: &[Run]) -> Vec<Finding>;
}

/// Where the catalogue comes from.
pub trait CatalogueSource {
    /// Why the catalogue could not be had.
    type Error: std::error::Error;
    /// The catalogue to use now.
    fn catalogue(&self) -> Result<Catalogue, Self::Error>;
}

/// The detectors of the public catalogue.
#[derive(Debug, Clone, Copy, Default)]
pub struct PublicDetector;

impl Detector for PublicDetector {
    fn detect(&self, runs: &[Run]) -> Vec<Finding> {
        public_findings(runs)
    }
}
