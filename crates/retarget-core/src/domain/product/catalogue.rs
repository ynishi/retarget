//! The catalogue: the techniques the product knows, as a versioned value.
//!
//! The catalogue is the contract between the Lab and the product. Each
//! technique says which pattern it answers, which step of which lever it
//! takes, what it is expected to do, when not to use it, how to tell whether
//! it fits a customer, and how it is rolled out. A public catalogue is built
//! from public facts; a tuned one is plugged in from outside the repository.
//! The domain receives a catalogue as a value; reading it from a file belongs
//! to an adapter.
//!
//! # Invariants
//!
//! - A technique's target position is one of its lever's positions, and it
//!   acts on at least one mechanism.
//! - An expected range, when there is one, is finite and ordered.
//! - Technique ids are unique within a catalogue.
//!
//! # Errors
//!
//! [`CatalogueError`] names the entry and the rule it broke; fix the entry.

use std::collections::BTreeSet;

use crate::domain::lever::Lever;
use crate::domain::measure::{Margins, StoppingRule, TrustLimits};
use crate::domain::mechanism::Mechanism;
use crate::domain::name::{PatternId, PositionId, Ratio, TechniqueId};

/// What a technique is expected to do to the cost per successful run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Expected {
    /// Nothing proven yet: a technique built from public facts alone.
    Unproven,
    /// The relative change the Lab measured, low to high (negative is cheaper).
    Range(f64, f64),
}

/// What happens when a window reaches its maximum without a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnUndecided {
    /// Keep the technique and go on watching it against the holdout.
    KeepWatching,
    /// Put the technique back.
    PutBack,
}

/// How far a technique is rolled out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    /// Shown to the customer, not applied.
    Shown,
    /// Applied to this share of runs.
    Partial(Ratio),
    /// Applied to all runs but a small holdout.
    Full,
}

/// How a technique is rolled out once allowed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rollout {
    /// Applied to a share of runs first, then to all.
    Partial(Ratio),
    /// Applied to all runs but the holdout at once.
    Full,
}

/// Where a technique comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Built from public facts.
    Public,
    /// Proven in the Lab.
    Lab,
}

/// How to tell whether a technique fits a customer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitSpec {
    /// How much worse (relative) the cost per successful run may get before
    /// the technique counts as not fitting.
    pub margin: Ratio,
    /// The share of runs left on the old position.
    pub holdout: Ratio,
    /// When a window stops.
    pub stopping: StoppingRule,
    /// The limits of the trust checks.
    pub trust: TrustLimits,
    /// How much worse a guardrail may get.
    pub margins: Margins,
    /// What happens at the maximum without a call.
    pub on_undecided: OnUndecided,
}

/// The parts of a technique, given to [`Technique::new`].
#[derive(Debug, Clone, PartialEq)]
pub struct TechniqueParts {
    /// Its stable name.
    pub id: TechniqueId,
    /// The pattern it answers.
    pub pattern: PatternId,
    /// The lever it moves.
    pub lever: Lever,
    /// Where it moves the lever to.
    pub to: PositionId,
    /// The mechanisms it acts on.
    pub mechanisms: BTreeSet<Mechanism>,
    /// What it is expected to do.
    pub expected: Expected,
    /// Patterns under which it must not be used.
    pub contraindications: Vec<PatternId>,
    /// How to tell whether it fits.
    pub fit: FitSpec,
    /// How it is rolled out once allowed.
    pub rollout: Rollout,
    /// Whether it is applied without asking (otherwise it waits, shown).
    pub automatic: bool,
    /// Where it comes from.
    pub source: Source,
}

/// A known fix for a pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct Technique {
    parts: TechniqueParts,
}

impl Technique {
    /// A technique, refused when it does not fit its lever or its range is broken.
    pub fn new(parts: TechniqueParts) -> Result<Self, CatalogueError> {
        if !parts.lever.positions().contains(&parts.to) {
            return Err(CatalogueError::UnknownPosition(parts.id));
        }
        if parts.mechanisms.is_empty() {
            return Err(CatalogueError::NoMechanism(parts.id));
        }
        if let Expected::Range(low, high) = parts.expected
            && !(low.is_finite() && high.is_finite() && low <= high)
        {
            return Err(CatalogueError::BadRange(parts.id));
        }
        Ok(Self { parts })
    }
    /// Its stable name.
    pub fn id(&self) -> &TechniqueId {
        &self.parts.id
    }
    /// The pattern it answers.
    pub fn pattern(&self) -> &PatternId {
        &self.parts.pattern
    }
    /// The lever it moves.
    pub fn lever(&self) -> &Lever {
        &self.parts.lever
    }
    /// Where it moves the lever to.
    pub fn to(&self) -> &PositionId {
        &self.parts.to
    }
    /// The mechanisms it acts on.
    pub fn mechanisms(&self) -> &BTreeSet<Mechanism> {
        &self.parts.mechanisms
    }
    /// What it is expected to do.
    pub fn expected(&self) -> Expected {
        self.parts.expected
    }
    /// Patterns under which it must not be used.
    pub fn contraindications(&self) -> &[PatternId] {
        &self.parts.contraindications
    }
    /// How to tell whether it fits.
    pub fn fit(&self) -> &FitSpec {
        &self.parts.fit
    }
    /// How it is rolled out once allowed.
    pub fn rollout(&self) -> Rollout {
        self.parts.rollout
    }
    /// Whether it is applied without asking.
    pub fn automatic(&self) -> bool {
        self.parts.automatic
    }
    /// Where it comes from.
    pub fn source(&self) -> Source {
        self.parts.source
    }
}

/// A versioned set of techniques.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalogue {
    version: u32,
    techniques: Vec<Technique>,
}

impl Catalogue {
    /// A catalogue, refused when two techniques share an id.
    pub fn new(version: u32, techniques: Vec<Technique>) -> Result<Self, CatalogueError> {
        let mut seen = BTreeSet::new();
        for technique in &techniques {
            if !seen.insert(technique.id().clone()) {
                return Err(CatalogueError::DuplicateId(technique.id().clone()));
            }
        }
        Ok(Self {
            version,
            techniques,
        })
    }
    /// Its version; a newer version may reopen techniques that did not fit.
    pub fn version(&self) -> u32 {
        self.version
    }
    /// The techniques for a pattern, in the catalogue's order of preference.
    pub fn for_pattern<'a>(
        &'a self,
        pattern: &'a PatternId,
    ) -> impl Iterator<Item = &'a Technique> {
        self.techniques
            .iter()
            .filter(move |t| t.pattern() == pattern)
    }
    /// A technique by id.
    pub fn get(&self, id: &TechniqueId) -> Option<&Technique> {
        self.techniques.iter().find(|t| t.id() == id)
    }
}

/// Why a catalogue entry was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogueError {
    /// The target position is not one of the lever's.
    #[error("technique `{0}` moves its lever to a position it does not have")]
    UnknownPosition(TechniqueId),
    /// The technique names no mechanism.
    #[error("technique `{0}` acts on no mechanism")]
    NoMechanism(TechniqueId),
    /// The expected range is not finite and ordered.
    #[error("technique `{0}` has a broken expected range")]
    BadRange(TechniqueId),
    /// Two techniques share an id.
    #[error("technique id `{0}` appears twice")]
    DuplicateId(TechniqueId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::testkit;

    #[test]
    fn entries_are_checked() {
        let mut parts = testkit::technique_parts();
        parts.to = "2h".parse().unwrap();
        assert!(matches!(
            Technique::new(parts),
            Err(CatalogueError::UnknownPosition(_))
        ));
        let mut parts = testkit::technique_parts();
        parts.expected = Expected::Range(0.1, -0.1);
        assert!(matches!(
            Technique::new(parts),
            Err(CatalogueError::BadRange(_))
        ));
        let t = Technique::new(testkit::technique_parts()).unwrap();
        assert!(matches!(
            Catalogue::new(1, vec![t.clone(), t]),
            Err(CatalogueError::DuplicateId(_))
        ));
    }
}
