//! An experiment: one step of one lever, with everything that decides it fixed
//! before it starts, and the windows of runs it is judged on.
//!
//! ```text
//! SpecParts ──> ExperimentSpec::new (step leads somewhere, hypothesis, limits fit)
//!                 └─ Ledger: evidence_class(hypothesis, demoted) ──> Experiment
//! Experiment ──> Window::trial | confirm | watch (who is control, who treatment, p)
//! ```
//!
//! # Invariants
//!
//! - Nothing of a spec changes once made: the threshold, the margins, the
//!   stopping rule and the shares are fixed before the change.
//! - The hypothesis comes only from [`Hypothesis::of`]; the kind of evidence
//!   only from [`evidence_class`].
//! - A window's probability of the treatment arm is its own, and it is the only
//!   place that probability is kept.
//! - Each arm can reach the minimum number of runs before the maximum: with
//!   probability `p`, `min_runs_per_arm ≤ min(p, 1−p) · max`.
//!
//! # Errors
//!
//! [`ExperimentError`] names the rule a spec broke; change the spec.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::cost::TermSet;
use crate::domain::lever::{Lever, LeverError, RoleScope, Step};
use crate::domain::mechanism::{EvidenceClass, Mechanism, effect_of, evidence_of};
use crate::domain::name::{ExperimentId, LeverKey, PositionId, Positive, Ratio, Share, WindowId};
use crate::domain::run::{Run, TaskClass};

/// The two arms of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    /// The position compared against.
    Control,
    /// The position under test.
    Treatment,
}

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

/// How much worse a guardrail may get.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Margins {
    /// Absolute drop of the success rate.
    pub success_rate: Ratio,
    /// Relative rise of the duration.
    pub duration: Ratio,
    /// Relative rise of the cost of rework.
    pub rework: Ratio,
}

/// How the error rate of one look is shared out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlphaSplit {
    primary: Ratio,
    guardrails: Ratio,
    trust: Ratio,
}

impl AlphaSplit {
    /// The split, refused when the parts add up to 1 or more.
    pub fn new(primary: Ratio, guardrails: Ratio, trust: Ratio) -> Result<Self, ExperimentError> {
        if primary.get() + guardrails.get() + trust.get() >= 1.0 {
            return Err(ExperimentError::AlphaTooLarge);
        }
        Ok(Self {
            primary,
            guardrails,
            trust,
        })
    }
    /// For the objective and the terms.
    pub fn primary(&self) -> f64 {
        self.primary.get()
    }
    /// For the three guardrails together.
    pub fn guardrails(&self) -> f64 {
        self.guardrails.get()
    }
    /// For the three trust checks together.
    pub fn trust(&self) -> f64 {
        self.trust.get()
    }
}

/// When an experiment stops.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoppingRule {
    rho: Positive,
    alpha: AlphaSplit,
    min_runs_per_arm: u32,
    min_successes_per_arm: u32,
    max_runs: u32,
    max_confirm_runs: u32,
}

/// The parts of a stopping rule, given to [`StoppingRule::new`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoppingParts {
    /// The tuning constant of the confidence sequences.
    pub rho: Positive,
    /// The error rate of one look, shared out.
    pub alpha: AlphaSplit,
    /// The runs each arm needs before anything is decided.
    pub min_runs_per_arm: u32,
    /// The successful runs each arm needs before anything is decided.
    pub min_successes_per_arm: u32,
    /// The runs of a trial after which it is reverted undecided.
    pub max_runs: u32,
    /// The runs of a confirmation after which it is reverted undecided.
    pub max_confirm_runs: u32,
}

impl StoppingRule {
    /// A stopping rule, refused when a minimum is zero or above a maximum.
    pub fn new(parts: StoppingParts) -> Result<Self, ExperimentError> {
        let StoppingParts {
            rho,
            alpha,
            min_runs_per_arm,
            min_successes_per_arm,
            max_runs,
            max_confirm_runs,
        } = parts;
        let min = u64::from(min_runs_per_arm) * 2;
        if min_runs_per_arm == 0
            || min_successes_per_arm > min_runs_per_arm
            || min > u64::from(max_runs)
            || min > u64::from(max_confirm_runs)
        {
            return Err(ExperimentError::MinAboveMax);
        }
        Ok(Self {
            rho,
            alpha,
            min_runs_per_arm,
            min_successes_per_arm,
            max_runs,
            max_confirm_runs,
        })
    }
    /// The tuning constant of the confidence sequences.
    pub fn rho(&self) -> f64 {
        self.rho.get()
    }
    /// The error rate of one look.
    pub fn alpha(&self) -> &AlphaSplit {
        &self.alpha
    }
    /// The runs each arm needs.
    pub fn min_runs_per_arm(&self) -> u32 {
        self.min_runs_per_arm
    }
    /// The successful runs each arm needs.
    pub fn min_successes_per_arm(&self) -> u32 {
        self.min_successes_per_arm
    }
    /// The runs of a trial at most.
    pub fn max_runs(&self) -> u32 {
        self.max_runs
    }
    /// The runs of a confirmation at most.
    pub fn max_confirm_runs(&self) -> u32 {
        self.max_confirm_runs
    }
}

/// The limits of the checks that must hold for a verdict to mean anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrustLimits {
    srm_concentration: Positive,
    effect_mismatch: Ratio,
    source_skew: Ratio,
    failure_streak: u32,
}

impl TrustLimits {
    /// The limits, refused when the failure streak is zero.
    pub fn new(
        srm_concentration: Positive,
        effect_mismatch: Ratio,
        source_skew: Ratio,
        failure_streak: u32,
    ) -> Result<Self, ExperimentError> {
        if failure_streak == 0 {
            return Err(ExperimentError::ZeroStreak);
        }
        Ok(Self {
            srm_concentration,
            effect_mismatch,
            source_skew,
            failure_streak,
        })
    }
    /// The prior concentration of the split test.
    pub fn srm_concentration(&self) -> f64 {
        self.srm_concentration.get()
    }
    /// The share of runs a change may fail to reach.
    pub fn effect_mismatch(&self) -> f64 {
        self.effect_mismatch.get()
    }
    /// How far the arms' shares of reported costs may differ.
    pub fn source_skew(&self) -> f64 {
        self.source_skew.get()
    }
    /// The failures in a row in the treatment arm that stop a trial.
    pub fn failure_streak(&self) -> u32 {
        self.failure_streak
    }
}

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

/// What a window is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowKind {
    /// The trial of an experiment.
    Trial(ExperimentId),
    /// The confirmation of a provisional keep against the holdout.
    Confirm(ExperimentId),
    /// The watch of a kept lever against the holdout.
    Watch(LeverKey),
}

/// The runs assigned with one probability, and what their arms mean.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    id: WindowId,
    kind: WindowKind,
    lever: LeverKey,
    control: PositionId,
    treatment: PositionId,
    p: Ratio,
    task_class: TaskClass,
    scope: RoleScope,
}

impl Window {
    fn of(id: WindowId, kind: WindowKind, experiment: &Experiment, p: Ratio) -> Self {
        let spec = experiment.spec();
        Self {
            id,
            kind,
            lever: spec.lever().key().clone(),
            control: spec.from().clone(),
            treatment: spec.to().clone(),
            p,
            task_class: spec.task_class().clone(),
            scope: spec.lever().scope().clone(),
        }
    }
    /// The trial: the new position for a share of runs.
    pub(crate) fn trial(id: WindowId, experiment: &Experiment) -> Self {
        let p = experiment.spec().trial_share();
        Self::of(id, WindowKind::Trial(experiment.spec().id()), experiment, p)
    }
    /// The confirmation: the new position for most runs, the old one for the holdout.
    pub(crate) fn confirm(id: WindowId, experiment: &Experiment) -> Self {
        Self::of(
            id,
            WindowKind::Confirm(experiment.spec().id()),
            experiment,
            holdout_complement(experiment),
        )
    }
    /// The watch: the same split as a confirmation, however the lever was kept.
    pub(crate) fn watch(id: WindowId, experiment: &Experiment) -> Self {
        Self::of(
            id,
            WindowKind::Watch(experiment.spec().lever().key().clone()),
            experiment,
            holdout_complement(experiment),
        )
    }
    /// Its id.
    pub fn id(&self) -> WindowId {
        self.id
    }
    /// What it is for.
    pub fn kind(&self) -> &WindowKind {
        &self.kind
    }
    /// The lever it assigns.
    pub fn lever(&self) -> &LeverKey {
        &self.lever
    }
    /// The position of the control arm.
    pub fn control(&self) -> &PositionId {
        &self.control
    }
    /// The position of the treatment arm.
    pub fn treatment(&self) -> &PositionId {
        &self.treatment
    }
    /// The probability of the treatment arm.
    pub fn p(&self) -> f64 {
        self.p.get()
    }
    /// The task class its runs must be in.
    pub fn task_class(&self) -> &TaskClass {
        &self.task_class
    }
    /// Which runs the lever acts on.
    pub fn scope(&self) -> &RoleScope {
        &self.scope
    }
    /// The arm a position belongs to, if any.
    pub fn arm_of(&self, position: &PositionId) -> Option<Arm> {
        if position == &self.treatment {
            Some(Arm::Treatment)
        } else if position == &self.control {
            Some(Arm::Control)
        } else {
            None
        }
    }
}

fn holdout_complement(experiment: &Experiment) -> Ratio {
    // 1 − holdout is strictly between 0 and 1 because the holdout is.
    Ratio::new(1.0 - experiment.spec().holdout_share().get())
        .unwrap_or(experiment.spec().holdout_share())
}

/// The window and position a run was assigned to for one lever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assigned {
    /// The window.
    pub window: WindowId,
    /// The position written for the run.
    pub position: PositionId,
}

/// A run and what it was assigned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedRun {
    /// The run.
    pub run: Run,
    /// Its assignments, per lever.
    pub assigned: BTreeMap<LeverKey, Assigned>,
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
    /// The parts of the error rate add up to 1 or more.
    #[error("the parts of alpha add up to 1 or more")]
    AlphaTooLarge,
    /// A failure streak of zero would stop every trial.
    #[error("the failure streak must be at least 1")]
    ZeroStreak,
    /// A minimum is zero or above a maximum.
    #[error("a minimum is zero or above its maximum")]
    MinAboveMax,
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
    fn limits_are_checked() {
        let r = |v| Ratio::new(v).unwrap();
        assert_eq!(
            AlphaSplit::new(r(0.5), r(0.3), r(0.2)),
            Err(ExperimentError::AlphaTooLarge)
        );
        let mut parts = testkit::stopping_parts();
        parts.min_runs_per_arm = 0;
        assert_eq!(StoppingRule::new(parts), Err(ExperimentError::MinAboveMax));
        assert!(TrustLimits::new(Positive::new(1.0).unwrap(), r(0.1), r(0.1), 0).is_err());
        let mut spec = testkit::spec_parts(testkit::ttl_lever());
        spec.holdout_share = r(0.005);
        assert_eq!(
            ExperimentSpec::new(spec),
            Err(ExperimentError::TooFewForMax)
        );
    }
}
