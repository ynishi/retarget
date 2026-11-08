//! An experiment of the Lab: one step of one lever, with everything that
//! decides it fixed before it starts, and the windows it is judged on.
//!
//! ```text
//! SpecParts ──> ExperimentSpec::new (step leads somewhere, hypothesis, limits fit)
//!                 └─ Ledger: evidence_class(hypothesis, demoted) ──> Experiment
//! Experiment ──> trial_window | confirm_window | watch_window
//! ```
//!
//! # Invariants
//!
//! - Nothing of a spec changes once made: the threshold, the margins, the
//!   stopping rule and the shares are fixed before the change.
//! - The hypothesis comes only from [`Hypothesis::of`]; the kind of evidence
//!   only from [`evidence_class`].
//! - Each arm can reach the minimum number of runs before the maximum: with
//!   probability `p`, `min_runs_per_arm ≤ min(p, 1−p) · max`.
//!
//! # Errors
//!
//! [`ExperimentError`] names the rule a spec broke; change the spec.

use std::collections::BTreeSet;

use crate::domain::assign::{Window, WindowKind, WindowParts};
use crate::domain::cost::TermSet;
use crate::domain::lever::{Lever, LeverError, Step};
use crate::domain::measure::{Margins, StoppingRule, TrustLimits};
use crate::domain::mechanism::{EvidenceClass, Mechanism, effect_of, evidence_of};
use crate::domain::name::{ExperimentId, PositionId, Ratio, Share, WindowId};
use crate::domain::run::TaskClass;

/// What a step is expected to do to the cost terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hypothesis {
    lowers: TermSet,
    may_raise: TermSet,
    mechanisms: BTreeSet<Mechanism>,
}

impl Hypothesis {
    /// The hypothesis of one step: the mechanisms the lever claims for that
    /// move, and what reducing them does. Nothing the lever did not claim
    /// enters it.
    pub fn of(lever: &Lever, step: &Step) -> Result<Self, ExperimentError> {
        let mechanisms: BTreeSet<Mechanism> = lever
            .claims()
            .iter()
            .filter(|claim| claim.reduced_when.matches(step))
            .map(|claim| claim.mechanism)
            .collect();
        if mechanisms.is_empty() {
            return Err(ExperimentError::NoClaimForStep);
        }
        let (mut lowers, mut may_raise) = (TermSet::empty(), TermSet::empty());
        for mechanism in &mechanisms {
            let effect = effect_of(*mechanism);
            lowers = lowers.union(effect.lowers());
            may_raise = may_raise.union(effect.may_raise());
        }
        Ok(Self {
            lowers: lowers.difference(&may_raise),
            may_raise,
            mechanisms,
        })
    }
    /// The terms that should fall.
    pub fn lowers(&self) -> &TermSet {
        &self.lowers
    }
    /// The terms that may rise.
    pub fn may_raise(&self) -> &TermSet {
        &self.may_raise
    }
    /// The mechanisms it rests on.
    pub fn mechanisms(&self) -> &BTreeSet<Mechanism> {
        &self.mechanisms
    }
}

/// The kind of evidence a hypothesis is judged on: priced only when every
/// mechanism only changes prices and none has lost the trust of its terms.
pub fn evidence_class(hypothesis: &Hypothesis, demoted: &BTreeSet<Mechanism>) -> EvidenceClass {
    let priced = hypothesis
        .mechanisms()
        .iter()
        .all(|m| evidence_of(*m) == EvidenceClass::Priced && !demoted.contains(m));
    if priced {
        EvidenceClass::Priced
    } else {
        EvidenceClass::Behavioral
    }
}

/// The relative improvement a trial must clear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Threshold(pub Ratio);

/// The relative improvement over the holdout a provisional keep must clear;
/// it may be zero, because the terms that cleared the threshold are only part
/// of the bill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfirmMargin(pub Share);

/// The parts of a spec, given to [`ExperimentSpec::new`].
#[derive(Debug, Clone, PartialEq)]
pub struct SpecParts {
    /// The caller's id for the experiment.
    pub id: ExperimentId,
    /// The lever.
    pub lever: Lever,
    /// Where it is now.
    pub from: PositionId,
    /// The step to try.
    pub step: Step,
    /// What a trial must clear.
    pub threshold: Threshold,
    /// What a confirmation must clear.
    pub confirm_margin: ConfirmMargin,
    /// When it stops.
    pub stopping: StoppingRule,
    /// How much worse a guardrail may get.
    pub margins: Margins,
    /// The limits of the trust checks.
    pub trust: TrustLimits,
    /// The share of a trial's runs that get the new position.
    pub trial_share: Ratio,
    /// The share of runs kept on the old position after a keep.
    pub holdout_share: Ratio,
    /// The task class it is judged in.
    pub task_class: TaskClass,
}

/// Everything that decides an experiment, fixed before it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentSpec {
    parts: SpecParts,
    to: PositionId,
    hypothesis: Hypothesis,
}

impl ExperimentSpec {
    /// A spec, refused when the step leads nowhere, nothing is claimed for it,
    /// or an arm could not reach its minimum before the maximum.
    pub fn new(parts: SpecParts) -> Result<Self, ExperimentError> {
        let to = parts.lever.apply(&parts.from, &parts.step)?;
        let hypothesis = Hypothesis::of(&parts.lever, &parts.step)?;
        let stopping = &parts.stopping;
        let reachable = |p: f64, max: u32| {
            f64::from(stopping.min_runs_per_arm()) <= p.min(1.0 - p) * f64::from(max)
        };
        if !reachable(parts.trial_share.get(), stopping.max_runs())
            || !reachable(1.0 - parts.holdout_share.get(), stopping.max_confirm_runs())
        {
            return Err(ExperimentError::TooFewForMax);
        }
        Ok(Self {
            parts,
            to,
            hypothesis,
        })
    }
    /// The caller's id.
    pub fn id(&self) -> ExperimentId {
        self.parts.id
    }
    /// The lever.
    pub fn lever(&self) -> &Lever {
        &self.parts.lever
    }
    /// Where it was.
    pub fn from(&self) -> &PositionId {
        &self.parts.from
    }
    /// Where the step leads.
    pub fn to(&self) -> &PositionId {
        &self.to
    }
    /// What the step is expected to do.
    pub fn hypothesis(&self) -> &Hypothesis {
        &self.hypothesis
    }
    /// What a trial must clear.
    pub fn threshold(&self) -> f64 {
        self.parts.threshold.0.get()
    }
    /// What a confirmation must clear.
    pub fn confirm_margin(&self) -> f64 {
        self.parts.confirm_margin.0.get()
    }
    /// When it stops.
    pub fn stopping(&self) -> &StoppingRule {
        &self.parts.stopping
    }
    /// How much worse a guardrail may get.
    pub fn margins(&self) -> &Margins {
        &self.parts.margins
    }
    /// The limits of the trust checks.
    pub fn trust(&self) -> &TrustLimits {
        &self.parts.trust
    }
    /// The share of a trial's runs on the new position.
    pub fn trial_share(&self) -> Ratio {
        self.parts.trial_share
    }
    /// The share of runs kept on the old position after a keep.
    pub fn holdout_share(&self) -> Ratio {
        self.parts.holdout_share
    }
    /// The task class.
    pub fn task_class(&self) -> &TaskClass {
        &self.parts.task_class
    }
}

/// An experiment the ledger started, with the evidence it is judged on.
#[derive(Debug, Clone, PartialEq)]
pub struct Experiment {
    spec: ExperimentSpec,
    evidence: EvidenceClass,
}

impl Experiment {
    /// Only the ledger makes one, because only the ledger knows which
    /// mechanisms have lost the trust of their terms.
    pub(crate) fn new(spec: ExperimentSpec, evidence: EvidenceClass) -> Self {
        Self { spec, evidence }
    }
    /// Its spec.
    pub fn spec(&self) -> &ExperimentSpec {
        &self.spec
    }
    /// The evidence it is judged on.
    pub fn evidence(&self) -> EvidenceClass {
        self.evidence
    }
}

fn window(id: WindowId, kind: WindowKind, experiment: &Experiment, p: Ratio) -> Window {
    let spec = experiment.spec();
    Window::new(WindowParts {
        id,
        kind,
        lever: spec.lever().key().clone(),
        control: spec.from().clone(),
        treatment: spec.to().clone(),
        p,
        task_class: Some(spec.task_class().clone()),
        scope: spec.lever().scope().clone(),
    })
}

fn holdout_complement(experiment: &Experiment) -> Ratio {
    // 1 − holdout is strictly between 0 and 1 because the holdout is.
    Ratio::new(1.0 - experiment.spec().holdout_share().get())
        .unwrap_or(experiment.spec().holdout_share())
}

/// The trial: the new position for a share of runs.
pub(crate) fn trial_window(id: WindowId, experiment: &Experiment) -> Window {
    let p = experiment.spec().trial_share();
    window(id, WindowKind::Trial(experiment.spec().id()), experiment, p)
}

/// The confirmation: the new position for most runs, the old one for the holdout.
pub(crate) fn confirm_window(id: WindowId, experiment: &Experiment) -> Window {
    let kind = WindowKind::Confirm(experiment.spec().id());
    window(id, kind, experiment, holdout_complement(experiment))
}

/// The watch: the same split as a confirmation, however the lever was kept.
pub(crate) fn watch_window(id: WindowId, experiment: &Experiment) -> Window {
    let kind = WindowKind::Watch(experiment.spec().lever().key().clone());
    window(id, kind, experiment, holdout_complement(experiment))
}

/// Why a spec was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentError {
    /// The step does not fit the lever.
    #[error(transparent)]
    Lever(#[from] LeverError),
    /// The lever claims no mechanism for this step.
    #[error("the lever claims no mechanism for this step")]
    NoClaimForStep,
    /// An arm could not reach its minimum before the maximum.
    #[error("an arm cannot reach its minimum runs before the maximum")]
    TooFewForMax,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::lever::Direction;
    use crate::domain::testkit;

    #[test]
    fn a_step_without_a_claim_has_no_hypothesis() {
        let lever = testkit::ttl_lever();
        assert_eq!(
            Hypothesis::of(&lever, &Step::Ordinal(Direction::Down)),
            Err(ExperimentError::NoClaimForStep)
        );
        let h = Hypothesis::of(&lever, &Step::Ordinal(Direction::Up)).unwrap();
        assert!(h.lowers().is_disjoint(h.may_raise()));
    }

    #[test]
    fn evidence_is_priced_only_for_price_mechanisms_that_kept_their_trust() {
        let h = Hypothesis::of(&testkit::ttl_lever(), &Step::Ordinal(Direction::Up)).unwrap();
        assert_eq!(evidence_class(&h, &BTreeSet::new()), EvidenceClass::Priced);
        let demoted = [Mechanism::CacheExpiry].into();
        assert_eq!(evidence_class(&h, &demoted), EvidenceClass::Behavioral);
    }

    #[test]
    fn an_arm_that_cannot_reach_its_minimum_is_refused() {
        let mut spec = testkit::spec_parts(testkit::ttl_lever());
        spec.holdout_share = Ratio::new(0.005).unwrap();
        assert_eq!(
            ExperimentSpec::new(spec),
            Err(ExperimentError::TooFewForMax)
        );
    }
}
