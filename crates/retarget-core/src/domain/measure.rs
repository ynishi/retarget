//! How a window of runs is measured, shared by the Lab and the product.
//!
//! The Lab proves a step and the product checks that a technique fits; both
//! pick the runs that count, pass the same gate, read the same guardrails and
//! compare with the same estimators, so those live here once.
//!
//! ```text
//! runs ──> eligible ──> gate ─────────────> guardrails ──> means / ratio_diff ──> objective_call
//!          (which runs  (failures in a row,  (not worse      (weighted arm means,  (worse, cleared,
//!           count, per   split, minimum,      than the        delta-method          no effect, open)
//!           measure)     maximum, trust)      margins)        sequences)
//! ```
//!
//! The estimators and their sources are in [`crate::domain::stats`]. A
//! guardrail whose measure is zero in every run of both arms (no rework at
//! all, say) cannot get worse, and counts as shown not worse: a sequence
//! around an exact zero never leaves zero.
//!
//! # Errors
//!
//! [`MeasureError`] names the limit a setting broke; change the setting.

use crate::domain::assign::{Arm, ObservedRun, Window};
use crate::domain::cost::{CostBreakdown, CostOrigin, TermSet, Work};
use crate::domain::lever::RoleScope;
use crate::domain::name::{Positive, Ratio};
use crate::domain::run::{Observed, Outcome, Span};
use crate::domain::stats::{Interval, SrmCheck, asymptotic_cs, cs_of, mean_var, sequential_srm};

/// Why a measuring setting was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MeasureError {
    /// The parts of the error rate add up to 1 or more.
    #[error("the parts of alpha add up to 1 or more")]
    AlphaTooLarge,
    /// A failure streak of zero would stop every window.
    #[error("the failure streak must be at least 1")]
    ZeroStreak,
    /// A minimum is zero or above its maximum.
    #[error("a minimum is zero or above its maximum")]
    MinAboveMax,
}

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
    pub fn new(primary: Ratio, guardrails: Ratio, trust: Ratio) -> Result<Self, MeasureError> {
        if primary.get() + guardrails.get() + trust.get() >= 1.0 {
            return Err(MeasureError::AlphaTooLarge);
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

/// When a window stops.
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
    /// The runs of a window after which it is closed undecided.
    pub max_runs: u32,
    /// The runs of a Lab confirmation after which it is closed undecided.
    pub max_confirm_runs: u32,
}

impl StoppingRule {
    /// A stopping rule, refused when a minimum is zero or above a maximum.
    pub fn new(parts: StoppingParts) -> Result<Self, MeasureError> {
        let min = u64::from(parts.min_runs_per_arm) * 2;
        if parts.min_runs_per_arm == 0
            || parts.min_successes_per_arm > parts.min_runs_per_arm
            || min > u64::from(parts.max_runs)
            || min > u64::from(parts.max_confirm_runs)
        {
            return Err(MeasureError::MinAboveMax);
        }
        Ok(Self {
            rho: parts.rho,
            alpha: parts.alpha,
            min_runs_per_arm: parts.min_runs_per_arm,
            min_successes_per_arm: parts.min_successes_per_arm,
            max_runs: parts.max_runs,
            max_confirm_runs: parts.max_confirm_runs,
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
    /// The runs of a window at most.
    pub fn max_runs(&self) -> u32 {
        self.max_runs
    }
    /// The runs of a Lab confirmation at most.
    pub fn max_confirm_runs(&self) -> u32 {
        self.max_confirm_runs
    }
}

/// The limits of the checks that must hold for a reading to mean anything.
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
    ) -> Result<Self, MeasureError> {
        if failure_streak == 0 {
            return Err(MeasureError::ZeroStreak);
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
    /// The failures in a row in the treatment arm that stop a window.
    pub fn failure_streak(&self) -> u32 {
        self.failure_streak
    }
}

/// How many runs each arm had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArmCounts {
    /// Runs of the control arm.
    pub control: u32,
    /// Runs of the treatment arm.
    pub treatment: u32,
}

/// How much worse each guardrail got beyond its margin: above zero is worse
/// than allowed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuardrailIntervals {
    /// The drop of the success rate beyond its margin.
    pub success_rate: Interval,
    /// The rise of the duration beyond its margin.
    pub duration: Interval,
    /// The rise of the cost of rework beyond its margin.
    pub rework: Interval,
}

/// The treatment's estimate against the control's, minus one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelativeChange(pub(crate) f64);

impl RelativeChange {
    /// The change; negative is cheaper.
    pub fn get(self) -> f64 {
        self.0
    }
}

/// A guardrail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guardrail {
    /// The success rate.
    SuccessRate,
    /// The duration of a run.
    Duration,
    /// The cost of rework.
    Rework,
}

/// A check that must hold for a reading to mean anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustBreach {
    /// The arms did not get the share of runs they were assigned.
    ArmImbalance,
    /// The change did not take effect in too many runs.
    NotTakingEffect,
    /// The arms' costs were taken from different sources.
    CostSourceSkew,
    /// Someone edited the lever's place in a file.
    ExternalEdit,
}

/// One run that counts.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub(crate) z: f64,
    pub(crate) success: f64,
    pub(crate) failure: bool,
    pub(crate) cost: f64,
    pub(crate) reported: f64,
    pub(crate) breakdown: CostBreakdown,
    pub(crate) duration: Option<f64>,
    pub(crate) effect_ok: Option<bool>,
}

/// The runs that count, by measure.
#[derive(Debug, Clone)]
pub(crate) struct Eligible {
    rows: Vec<Row>,
}

/// A measure read from a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Measure {
    Objective,
    Terms,
    Duration,
}

impl Eligible {
    pub(crate) fn of(&self, measure: Measure) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|row| match measure {
                Measure::Objective => true,
                Measure::Terms => row.breakdown.has_terms(),
                Measure::Duration => row.duration.is_some(),
            })
            .collect()
    }
}

pub(crate) fn counts(rows: &[&Row]) -> ArmCounts {
    let treatment = rows.iter().filter(|row| row.z == 1.0).count() as u32;
    ArmCounts {
        control: rows.len() as u32 - treatment,
        treatment,
    }
}

/// Which runs of a window count. This is the one place that decides it: a run
/// counts when it was assigned in this window to one of its two positions,
/// kept one configuration, is in the window's task class (if it has one), has
/// a known outcome and a countable cost, and (for a lever scoped to a role)
/// has that role. Each measure then keeps the runs that have it.
pub(crate) fn eligible(window: &Window, runs: &[ObservedRun]) -> Eligible {
    let rows = runs
        .iter()
        .filter_map(|observed| {
            let assigned = observed.assigned.get(window.lever())?;
            if assigned.window != window.id() {
                return None;
            }
            let arm = window.arm_of(&assigned.position)?;
            let run = &observed.run;
            if run.span() != Span::Within || run.outcome() == Outcome::Unknown {
                return None;
            }
            if let Some(class) = window.task_class()
                && run.task_class() != class
            {
                return None;
            }
            if let RoleScope::Role(role) = window.scope()
                && !run.has_role(role)
            {
                return None;
            }
            let breakdown = CostBreakdown::of(run).ok()?;
            let objective = breakdown.objective();
            let effect_ok = match run.start().effective.get(window.lever()) {
                Some(Observed::Known(position)) => Some(position == &assigned.position),
                _ => None,
            };
            Some(Row {
                z: if arm == Arm::Treatment { 1.0 } else { 0.0 },
                success: if run.outcome() == Outcome::Success {
                    1.0
                } else {
                    0.0
                },
                failure: run.outcome() == Outcome::Failure,
                cost: objective.cost.micros() as f64,
                reported: if objective.origin == CostOrigin::Reported {
                    1.0
                } else {
                    0.0
                },
                breakdown,
                duration: run.duration_ms().known().map(|ms| *ms as f64),
                effect_ok,
            })
        })
        .collect();
    Eligible { rows }
}

/// What the gate lets through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Gate {
    Proceed,
    Tripped,
    Halted(TrustBreach),
    Pending,
    /// Too few runs, and the maximum is reached.
    Exhausted,
}

/// The checks every window passes before its measures are read.
pub(crate) fn gate(
    window: &Window,
    eligible: &Eligible,
    measures: &[Measure],
    stopping: &StoppingRule,
    trust: &TrustLimits,
    max: Option<u32>,
) -> Gate {
    let all = eligible.of(Measure::Objective);
    // (a) failures in a row in the treatment arm
    let mut streak = 0;
    for row in all.iter().filter(|row| row.z == 1.0) {
        streak = if row.failure { streak + 1 } else { 0 };
        if streak >= trust.failure_streak() {
            return Gate::Tripped;
        }
    }
    // (b) the split of every measure that will be read; valid at any time
    let alpha_trust = stopping.alpha().trust() / 3.0;
    let alpha_srm = alpha_trust / measures.len().max(1) as f64;
    for measure in measures {
        let c = counts(&eligible.of(*measure));
        if let SrmCheck::Broken(_) = sequential_srm(
            u64::from(c.treatment),
            u64::from(c.control),
            window.p(),
            trust.srm_concentration(),
            alpha_srm,
        ) {
            return Gate::Halted(TrustBreach::ArmImbalance);
        }
    }
    // (c) enough runs of every measure, and successes, before the maximum
    let min = stopping.min_runs_per_arm();
    let short = measures.iter().any(|measure| {
        let c = counts(&eligible.of(*measure));
        c.control < min || c.treatment < min
    });
    let successes = |z: f64| {
        all.iter()
            .filter(|row| row.z == z && row.success == 1.0)
            .count() as u32
    };
    let few_successes = successes(0.0) < stopping.min_successes_per_arm()
        || successes(1.0) < stopping.min_successes_per_arm();
    if short || few_successes {
        return match max {
            Some(max) if all.len() as u32 >= max => Gate::Exhausted,
            _ => Gate::Pending,
        };
    }
    // (d) the rates of the trust checks, read as sequences
    let look = Look::new(stopping.rho(), alpha_trust);
    let effect: Vec<f64> = all
        .iter()
        .filter_map(|row| row.effect_ok.map(|ok| if ok { 0.0 } else { 1.0 }))
        .collect();
    if cs_of(&effect, look.rho, look.alpha).lower() > trust.effect_mismatch() {
        return Gate::Halted(TrustBreach::NotTakingEffect);
    }
    // share reported in control minus share reported in treatment
    let skew = means(&all, window.p(), 1.0, 1.0, &|r| r.reported, &|_| 1.0, look).interval;
    if skew.lower() > trust.source_skew() || skew.upper() < -trust.source_skew() {
        return Gate::Halted(TrustBreach::CostSourceSkew);
    }
    Gate::Proceed
}

/// The error rate and tuning of one reading.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Look {
    pub(crate) rho: f64,
    pub(crate) alpha: f64,
}

impl Look {
    pub(crate) fn new(rho: f64, alpha: f64) -> Self {
        Self { rho, alpha }
    }
}

/// An estimate of `a·R_control − b·R_treatment`, where `R` is the ratio of the
/// means of `num` and `den` in an arm.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Estimate {
    pub(crate) interval: Interval,
    pub(crate) control: f64,
    pub(crate) treatment: f64,
}

/// Ratios of arm means, weighted by the known probability `p`, with the delta
/// method's influence for the interval. With `den = 1` it is a difference of
/// means.
pub(crate) fn means(
    rows: &[&Row],
    p: f64,
    a: f64,
    b: f64,
    num: &dyn Fn(&Row) -> f64,
    den: &dyn Fn(&Row) -> f64,
    look: Look,
) -> Estimate {
    let n = rows.len() as f64;
    let unknown = Estimate {
        interval: Interval::unbounded(),
        control: 0.0,
        treatment: 0.0,
    };
    if rows.is_empty() {
        return unknown;
    }
    let arm = |z: f64, w: f64| {
        let pick = rows.iter().filter(|r| r.z == z);
        let x: f64 = pick.clone().map(|r| num(r) / w).sum::<f64>() / n;
        let y: f64 = pick.map(|r| den(r) / w).sum::<f64>() / n;
        (x, y)
    };
    let (xc, yc) = arm(0.0, 1.0 - p);
    let (xt, yt) = arm(1.0, p);
    if yc <= 0.0 || yt <= 0.0 {
        return unknown;
    }
    let (rc, rt) = (xc / yc, xt / yt);
    let psi: Vec<f64> = rows
        .iter()
        .map(|r| {
            a * (1.0 - r.z) / (1.0 - p) * (num(r) - rc * den(r)) / yc
                - b * r.z / p * (num(r) - rt * den(r)) / yt
        })
        .collect();
    let (_, var) = mean_var(&psi);
    Estimate {
        interval: asymptotic_cs(a * rc - b * rt, var, rows.len(), look.rho, look.alpha),
        control: rc,
        treatment: rt,
    }
}

/// `a·R_control − b·R_treatment` of the cost per successful run.
pub(crate) fn ratio_diff(window: &Window, rows: &[&Row], a: f64, b: f64, look: Look) -> Estimate {
    means(rows, window.p(), a, b, &|r| r.cost, &|r| r.success, look)
}

/// The treatment's estimate against the control's, minus one.
pub(crate) fn relative(estimate: &Estimate) -> RelativeChange {
    if estimate.control > 0.0 {
        RelativeChange(estimate.treatment / estimate.control - 1.0)
    } else {
        RelativeChange(0.0)
    }
}

/// The guardrails, judged together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GuardrailCheck {
    Breached(Guardrail, GuardrailIntervals),
    NonInferior(GuardrailIntervals),
    Undecided(GuardrailIntervals),
}

impl GuardrailCheck {
    pub(crate) fn intervals(&self) -> GuardrailIntervals {
        match self {
            GuardrailCheck::Breached(_, i)
            | GuardrailCheck::NonInferior(i)
            | GuardrailCheck::Undecided(i) => *i,
        }
    }
}

fn negate(i: Interval) -> Interval {
    Interval::new(-i.upper(), -i.lower()).unwrap_or_else(Interval::unbounded)
}

/// How much worse each guardrail got beyond its margin.
pub(crate) fn guardrails(
    window: &Window,
    eligible: &Eligible,
    stopping: &StoppingRule,
    margins: &Margins,
) -> GuardrailCheck {
    let p = window.p();
    let look = Look::new(stopping.rho(), stopping.alpha().guardrails() / 3.0);
    let one = |_: &Row| 1.0;
    let all = eligible.of(Measure::Objective);
    // (rate_control − rate_treatment) − margin
    let drop = means(&all, p, 1.0, 1.0, &|r| r.success, &one, look).interval;
    let margin = margins.success_rate.get();
    let success_rate = Interval::new(drop.lower() - margin, drop.upper() - margin)
        .unwrap_or_else(Interval::unbounded);
    // μ_treatment − (1 + margin)·μ_control
    let rise = |rows: &[&Row], m: &dyn Fn(&Row) -> f64, margin: f64| {
        let trivial = rows.iter().all(|r| m(r) == 0.0);
        (
            negate(means(rows, p, 1.0 + margin, 1.0, m, &one, look).interval),
            trivial,
        )
    };
    let (duration, _) = rise(
        &eligible.of(Measure::Duration),
        &|r| r.duration.unwrap_or(0.0),
        margins.duration.get(),
    );
    let rework_set = TermSet::of_work(Work::Rework);
    let (rework, no_rework) = rise(
        &eligible.of(Measure::Terms),
        &|r| {
            r.breakdown
                .sum(&rework_set)
                .known()
                .map_or(0.0, |c| c.micros() as f64)
        },
        margins.rework.get(),
    );
    let intervals = GuardrailIntervals {
        success_rate,
        duration,
        rework,
    };
    let named = [
        (Guardrail::SuccessRate, success_rate, false),
        (Guardrail::Duration, duration, false),
        (Guardrail::Rework, rework, no_rework),
    ];
    if let Some((guardrail, _, _)) = named
        .iter()
        .find(|(_, i, trivial)| !trivial && i.lower() > 0.0)
    {
        GuardrailCheck::Breached(*guardrail, intervals)
    } else if named
        .iter()
        .all(|(_, i, trivial)| *trivial || i.upper() < 0.0)
    {
        GuardrailCheck::NonInferior(intervals)
    } else {
        GuardrailCheck::Undecided(intervals)
    }
}

/// What the objective says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectiveCall {
    Worse,
    Cleared,
    NoEffect,
    Undecided,
}

/// The one reading of the objective: `worse` is `R_control − R_treatment`,
/// `clears` is the quantity that must be above zero.
pub(crate) fn objective_call(worse: &Interval, clears: &Interval) -> ObjectiveCall {
    if worse.upper() < 0.0 {
        ObjectiveCall::Worse
    } else if clears.lower() > 0.0 {
        ObjectiveCall::Cleared
    } else if clears.upper() < 0.0 {
        ObjectiveCall::NoEffect
    } else {
        ObjectiveCall::Undecided
    }
}

/// The sum of a set of cells of a row, zero when the row has no cells.
pub(crate) fn sum_of(set: &TermSet) -> impl Fn(&Row) -> f64 + '_ {
    move |row| {
        row.breakdown
            .sum(set)
            .known()
            .map_or(0.0, |c| c.micros() as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_objective_is_read_in_one_order() {
        let i = |l, u| Interval::new(l, u).unwrap();
        assert_eq!(
            objective_call(&i(-3.0, -1.0), &i(1.0, 2.0)),
            ObjectiveCall::Worse
        );
        assert_eq!(
            objective_call(&i(-1.0, 1.0), &i(1.0, 2.0)),
            ObjectiveCall::Cleared
        );
        assert_eq!(
            objective_call(&i(-1.0, 1.0), &i(-2.0, -1.0)),
            ObjectiveCall::NoEffect
        );
        assert_eq!(
            objective_call(&i(-1.0, 1.0), &i(-1.0, 1.0)),
            ObjectiveCall::Undecided
        );
    }

    #[test]
    fn settings_keep_their_limits() {
        let r = |v| Ratio::new(v).unwrap();
        assert_eq!(
            AlphaSplit::new(r(0.5), r(0.3), r(0.2)),
            Err(MeasureError::AlphaTooLarge)
        );
        let mut parts = crate::domain::testkit::stopping_parts();
        parts.min_runs_per_arm = 0;
        assert_eq!(StoppingRule::new(parts), Err(MeasureError::MinAboveMax));
        assert!(TrustLimits::new(Positive::new(1.0).unwrap(), r(0.1), r(0.1), 0).is_err());
    }
}
