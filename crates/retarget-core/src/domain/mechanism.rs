//! Mechanisms: the ways a lever moves cost terms, and the evidence each needs.
//!
//! Which terms a mechanism lowers, which it may raise, and whether moving it
//! only changes prices are knowledge of this domain, not of any harness. They
//! live in one table, [`effect_of`] and [`evidence_of`], read by nothing but
//! the construction of a hypothesis and the choice of evidence.
//!
//! | mechanism | lowers | may raise | evidence |
//! |---|---|---|---|
//! | cache expiry | cache writes | cache reads | priced |
//! | cache break | cache writes, uncached input | — | priced |
//! | context growth | cache reads, uncached input | rework | behavioural |
//! | off-main duplication | off-main work | main work | behavioural |
//! | reasoning volume | output | rework | behavioural |
//! | model price | all first work | rework | behavioural |
//! | rework | rework | output | behavioural |
//!
//! "Lowers" excludes rework unless the row says rework, and what a mechanism
//! lowers never overlaps what it may raise.

use crate::domain::cost::{Segment, TermSet, Work};
use crate::domain::money::TokenKind;

/// A way a lever moves the cost of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mechanism {
    /// A cache that expires and is written again.
    CacheExpiry,
    /// An operation that throws the cache away.
    CacheBreak,
    /// Context that grows.
    ContextGrowth,
    /// The same context handed to several subagents.
    OffMainDuplication,
    /// The volume of reasoning.
    ReasoningVolume,
    /// The price of the model a role gets.
    ModelPrice,
    /// Work redone after a failure.
    Rework,
}

impl Mechanism {
    /// Every mechanism.
    pub const ALL: [Mechanism; 7] = [
        Mechanism::CacheExpiry,
        Mechanism::CacheBreak,
        Mechanism::ContextGrowth,
        Mechanism::OffMainDuplication,
        Mechanism::ReasoningVolume,
        Mechanism::ModelPrice,
        Mechanism::Rework,
    ];
}

/// The kind of evidence a lever is judged on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvidenceClass {
    /// The lever only changes what the same work costs: judged on the cost
    /// terms its mechanisms move, and kept provisionally.
    Priced,
    /// The lever changes how the agent works: judged on the cost per
    /// successful run directly.
    Behavioral,
}

/// What reducing a mechanism does to the cost terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MechanismEffect {
    lowers: TermSet,
    may_raise: TermSet,
}

impl MechanismEffect {
    /// The terms that should fall.
    pub fn lowers(&self) -> &TermSet {
        &self.lowers
    }
    /// The terms that may rise and eat the saving.
    pub fn may_raise(&self) -> &TermSet {
        &self.may_raise
    }
}

fn first(set: TermSet) -> TermSet {
    set.intersection(&TermSet::of_work(Work::First))
}

/// What reducing a mechanism lowers, and what it may raise.
pub fn effect_of(mechanism: Mechanism) -> MechanismEffect {
    let kind = TermSet::of_kind;
    let rework = TermSet::of_work(Work::Rework);
    let (lowers, may_raise) = match mechanism {
        Mechanism::CacheExpiry => (
            first(kind(TokenKind::CacheWrite)),
            first(kind(TokenKind::CacheRead)),
        ),
        Mechanism::CacheBreak => (
            first(kind(TokenKind::CacheWrite).union(&kind(TokenKind::UncachedInput))),
            TermSet::empty(),
        ),
        Mechanism::ContextGrowth => (
            first(kind(TokenKind::CacheRead).union(&kind(TokenKind::UncachedInput))),
            rework,
        ),
        Mechanism::OffMainDuplication => (
            first(TermSet::of_segment(Segment::OffMain)),
            first(TermSet::of_segment(Segment::Main)),
        ),
        Mechanism::ReasoningVolume => (first(kind(TokenKind::Output)), rework),
        Mechanism::ModelPrice => (TermSet::of_work(Work::First), rework),
        Mechanism::Rework => (rework, first(kind(TokenKind::Output))),
    };
    MechanismEffect { lowers, may_raise }
}

/// The evidence a mechanism allows: prices only, or behaviour.
pub fn evidence_of(mechanism: Mechanism) -> EvidenceClass {
    match mechanism {
        Mechanism::CacheExpiry | Mechanism::CacheBreak => EvidenceClass::Priced,
        Mechanism::ContextGrowth
        | Mechanism::OffMainDuplication
        | Mechanism::ReasoningVolume
        | Mechanism::ModelPrice
        | Mechanism::Rework => EvidenceClass::Behavioral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_falls_never_overlaps_what_may_rise() {
        for mechanism in Mechanism::ALL {
            let effect = effect_of(mechanism);
            assert!(!effect.lowers().is_empty(), "{mechanism:?} lowers nothing");
            assert!(
                effect.lowers().is_disjoint(effect.may_raise()),
                "{mechanism:?} overlaps"
            );
        }
    }

    #[test]
    fn only_cache_mechanisms_are_priced() {
        let priced: Vec<_> = Mechanism::ALL
            .into_iter()
            .filter(|m| evidence_of(*m) == EvidenceClass::Priced)
            .collect();
        assert_eq!(priced, [Mechanism::CacheExpiry, Mechanism::CacheBreak]);
    }
}
