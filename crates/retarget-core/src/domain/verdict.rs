//! Verdicts: what a window of runs says about a step.
//!
//! Three questions are asked of three kinds of window, and they share every
//! part but the last:
//!
//! ```text
//! runs ──> eligible ──> gate ─────────────> guardrails ──> the objective ──> result
//!          (which runs  (failures in a row,  (not worse      objective_call
//!           count, per   split, minimum,      than the        (worse, cleared,
//!           measure)     maximum, trust)      margins)         no effect, open)
//!
//! evaluate: a trial                                 ──> Verdict
//! confirm:  a provisional keep against the holdout  ──> Confirmation
//! watch:    a kept lever against the holdout        ──> WatchVerdict
//! ```
//!
//! The rules are stated in [`crate::domain`], the estimators in
//! [`crate::domain::stats`]. Inside `evaluate` the order is: the gate (a run
//! of failures, a broken split, too few runs, a broken trust check), then a
//! guardrail clearly worse, then the objective clearly worse, then cost terms
//! that fell while the bill did not (shifted), then the threshold cleared with
//! every guardrail shown not worse, then no effect shown, then the maximum,
//! else pending. A failure streak comes first because it harms the user
//! whatever its cause.
//!
//! A guardrail whose measure is zero in every run of both arms (no rework at
//! all, say) cannot get worse, and counts as shown not worse: a sequence
//! around an exact zero never leaves zero.

use crate::domain::cost::{CostBreakdown, CostOrigin, TermSet, Work};
use crate::domain::experiment::{
    Arm, Experiment, Margins, ObservedRun, StoppingRule, TrustLimits, Window,
};
use crate::domain::lever::RoleScope;
use crate::domain::mechanism::EvidenceClass;
use crate::domain::run::{Observed, Outcome, Span};
use crate::domain::stats::{Interval, SrmCheck, asymptotic_cs, cs_of, mean_var, sequential_srm};

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

/// The evidence a trial was judged on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EvidenceDetail {
    /// The cost terms of the hypothesis.
    Priced {
        /// `(1−δ)·μ_control − μ_treatment` of the terms that should fall.
        lowers: Interval,
        /// `μ_control − μ_treatment` of those and the terms that may rise.
        net: Interval,
    },
    /// The cost per successful run directly.
    Behavioral,
}

/// The treatment's estimate against the control's, minus one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelativeChange(f64);

impl RelativeChange {
    /// The change; negative is cheaper.
    pub fn get(self) -> f64 {
        self.0
    }
}

/// What a trial or a confirmation measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Response {
    /// The runs counted, per arm.
    pub counts: ArmCounts,
    /// `R_control − R_treatment` of the cost per successful run, in micro dollars.
    pub objective: Interval,
    /// The treatment's cost per successful run against the control's.
    pub relative: RelativeChange,
    /// The evidence the step was judged on.
    pub evidence: EvidenceDetail,
    /// How the guardrails moved.
    pub guardrails: GuardrailIntervals,
}

/// What a watch measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatchResponse {
    /// The runs counted, per arm.
    pub counts: ArmCounts,
    /// `R_holdout − R_kept` of the cost per successful run.
    pub objective: Interval,
    /// The kept position's cost per successful run against the holdout's.
    pub relative: RelativeChange,
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

/// A check that must hold for a verdict to mean anything.
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

/// Why a change is put back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevertReason {
    /// It was shown not to clear its threshold.
    NoEffect,
    /// The terms it should lower fell, and the bill did not.
    Shifted,
    /// The cost per successful run was shown to rise.
    Worse,
    /// A guardrail was shown worse than its margin.
    GuardrailBreached(Guardrail),
    /// It reached its maximum of runs undecided.
    Exhausted,
}

/// The verdict on a trial.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// More runs are needed.
    Pending,
    /// The cost terms cleared the threshold; confirm against the holdout.
    ProvisionalKeep(Response),
    /// The cost per successful run cleared the threshold.
    Keep(Response),
    /// Put the change back.
    Revert(RevertReason, Response),
    /// Too many failures in a row.
    Tripped,
    /// A trust check broke; no verdict.
    Halted(TrustBreach),
}

/// The verdict on a confirmation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Confirmation {
    /// More runs are needed.
    Pending,
    /// The saving shows in the bill.
    Keep(Response),
    /// Put the change back.
    Revert(RevertReason, Response),
    /// Too many failures in a row.
    Tripped,
    /// A trust check broke; no verdict.
    Halted(TrustBreach),
}

/// The verdict of a watch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WatchVerdict {
    /// The kept position is not shown worse than the holdout.
    Holding(WatchResponse),
    /// More runs are needed.
    Pending,
    /// The kept position is shown worse than the holdout.
    Worse(WatchResponse),
    /// Too many failures in a row.
    Tripped,
    /// A trust check broke.
    Halted(TrustBreach),
}

/// One run that counts.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    z: f64,
    success: f64,
    failure: bool,
    cost: f64,
    reported: f64,
    breakdown: CostBreakdown,
    duration: Option<f64>,
    effect_ok: Option<bool>,
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
    fn of(&self, measure: Measure) -> Vec<&Row> {
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

fn counts(rows: &[&Row]) -> ArmCounts {
    let treatment = rows.iter().filter(|row| row.z == 1.0).count() as u32;
    ArmCounts {
        control: rows.len() as u32 - treatment,
        treatment,
    }
}

/// Which runs of a window count. This is the one place that decides it: a run
/// counts when it was assigned in this window to one of its two positions,
/// kept one configuration, is in the task class, has a known outcome and a
/// countable cost, and (for a lever scoped to a role) has that role. Each
/// measure then keeps the runs that have it.
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
            if run.span() != Span::Within
                || run.task_class() != window.task_class()
                || run.outcome() == Outcome::Unknown
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
    rho: f64,
    alpha: f64,
}

impl Look {
    fn new(rho: f64, alpha: f64) -> Self {
        Self { rho, alpha }
    }
}

/// An estimate of `a·R_control − b·R_treatment`, where `R` is the ratio of the
/// means of `num` and `den` in an arm.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Estimate {
    interval: Interval,
    control: f64,
    treatment: f64,
}

/// Ratios of arm means, weighted by the known probability `p`, with the delta
/// method's influence for the interval. With `den = 1` it is a difference of
/// means.
fn means(
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

fn relative(estimate: &Estimate) -> RelativeChange {
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
    fn intervals(&self) -> GuardrailIntervals {
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

const MEASURES: [Measure; 3] = [Measure::Objective, Measure::Duration, Measure::Terms];

fn sum_of(set: &TermSet) -> impl Fn(&Row) -> f64 + '_ {
    move |row| {
        row.breakdown
            .sum(set)
            .known()
            .map_or(0.0, |c| c.micros() as f64)
    }
}

/// The verdict on a trial.
pub fn evaluate(experiment: &Experiment, window: &Window, runs: &[ObservedRun]) -> Verdict {
    let spec = experiment.spec();
    let stopping = spec.stopping();
    let rows = eligible(window, runs);
    let gate = gate(
        window,
        &rows,
        &MEASURES,
        stopping,
        spec.trust(),
        Some(stopping.max_runs()),
    );
    match gate {
        Gate::Proceed | Gate::Exhausted => {}
        Gate::Tripped => return Verdict::Tripped,
        Gate::Halted(breach) => return Verdict::Halted(breach),
        Gate::Pending => return Verdict::Pending,
    }
    let all = rows.of(Measure::Objective);
    let at_max = all.len() as u32 >= stopping.max_runs();
    let look = Look::new(stopping.rho(), stopping.alpha().primary());
    let guard = guardrails(window, &rows, stopping, spec.margins());
    let objective = ratio_diff(window, &all, 1.0, 1.0, look);
    let delta = spec.threshold();
    let (clears, evidence) = match experiment.evidence() {
        EvidenceClass::Priced => {
            let terms = rows.of(Measure::Terms);
            let h = spec.hypothesis();
            let net_set = h.lowers().union(h.may_raise());
            let one = |_: &Row| 1.0;
            let lowers = means(
                &terms,
                window.p(),
                1.0 - delta,
                1.0,
                &sum_of(h.lowers()),
                &one,
                look,
            )
            .interval;
            let net = means(&terms, window.p(), 1.0, 1.0, &sum_of(&net_set), &one, look).interval;
            (lowers, EvidenceDetail::Priced { lowers, net })
        }
        EvidenceClass::Behavioral => (
            ratio_diff(window, &all, 1.0 - delta, 1.0, look).interval,
            EvidenceDetail::Behavioral,
        ),
    };
    let response = Response {
        counts: counts(&all),
        objective: objective.interval,
        relative: relative(&objective),
        evidence,
        guardrails: guard.intervals(),
    };
    if let GuardrailCheck::Breached(g, _) = guard {
        return Verdict::Revert(RevertReason::GuardrailBreached(g), response);
    }
    let call = objective_call(&objective.interval, &clears);
    if call == ObjectiveCall::Worse {
        return Verdict::Revert(RevertReason::Worse, response);
    }
    if let EvidenceDetail::Priced { lowers, net } = evidence
        && lowers.lower() > 0.0
        && net.upper() < 0.0
    {
        return Verdict::Revert(RevertReason::Shifted, response);
    }
    let net_ok = match evidence {
        EvidenceDetail::Priced { net, .. } => net.lower() > 0.0,
        EvidenceDetail::Behavioral => true,
    };
    if call == ObjectiveCall::Cleared && net_ok && matches!(guard, GuardrailCheck::NonInferior(_)) {
        return match experiment.evidence() {
            EvidenceClass::Priced => Verdict::ProvisionalKeep(response),
            EvidenceClass::Behavioral => Verdict::Keep(response),
        };
    }
    if call == ObjectiveCall::NoEffect {
        return Verdict::Revert(RevertReason::NoEffect, response);
    }
    if at_max || gate == Gate::Exhausted {
        return Verdict::Revert(RevertReason::Exhausted, response);
    }
    Verdict::Pending
}

/// The verdict on a confirmation against the holdout.
pub fn confirm(experiment: &Experiment, window: &Window, runs: &[ObservedRun]) -> Confirmation {
    let spec = experiment.spec();
    let stopping = spec.stopping();
    let max = stopping.max_confirm_runs();
    let rows = eligible(window, runs);
    let gate = gate(window, &rows, &MEASURES, stopping, spec.trust(), Some(max));
    match gate {
        Gate::Proceed | Gate::Exhausted => {}
        Gate::Tripped => return Confirmation::Tripped,
        Gate::Halted(breach) => return Confirmation::Halted(breach),
        Gate::Pending => return Confirmation::Pending,
    }
    let all = rows.of(Measure::Objective);
    let look = Look::new(stopping.rho(), stopping.alpha().primary());
    let guard = guardrails(window, &rows, stopping, spec.margins());
    let objective = ratio_diff(window, &all, 1.0, 1.0, look);
    let clears = ratio_diff(window, &all, 1.0 - spec.confirm_margin(), 1.0, look).interval;
    let response = Response {
        counts: counts(&all),
        objective: objective.interval,
        relative: relative(&objective),
        evidence: EvidenceDetail::Behavioral,
        guardrails: guard.intervals(),
    };
    if let GuardrailCheck::Breached(g, _) = guard {
        return Confirmation::Revert(RevertReason::GuardrailBreached(g), response);
    }
    match objective_call(&objective.interval, &clears) {
        ObjectiveCall::Worse => Confirmation::Revert(RevertReason::Worse, response),
        ObjectiveCall::Cleared if matches!(guard, GuardrailCheck::NonInferior(_)) => {
            Confirmation::Keep(response)
        }
        ObjectiveCall::NoEffect => Confirmation::Revert(RevertReason::NoEffect, response),
        _ if gate == Gate::Exhausted || all.len() as u32 >= max => {
            Confirmation::Revert(RevertReason::Exhausted, response)
        }
        _ => Confirmation::Pending,
    }
}

/// The verdict of a watch over a kept lever.
pub fn watch(
    window: &Window,
    stopping: &StoppingRule,
    trust: &TrustLimits,
    runs: &[ObservedRun],
) -> WatchVerdict {
    let rows = eligible(window, runs);
    match gate(window, &rows, &[Measure::Objective], stopping, trust, None) {
        Gate::Proceed | Gate::Exhausted => {}
        Gate::Tripped => return WatchVerdict::Tripped,
        Gate::Halted(breach) => return WatchVerdict::Halted(breach),
        Gate::Pending => return WatchVerdict::Pending,
    }
    let all = rows.of(Measure::Objective);
    let look = Look::new(stopping.rho(), stopping.alpha().primary());
    let objective = ratio_diff(window, &all, 1.0, 1.0, look);
    let response = WatchResponse {
        counts: counts(&all),
        objective: objective.interval,
        relative: relative(&objective),
    };
    match objective_call(&objective.interval, &Interval::unbounded()) {
        ObjectiveCall::Worse => WatchVerdict::Worse(response),
        _ => WatchVerdict::Holding(response),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::experiment::{ExperimentSpec, evidence_class};
    use crate::domain::lever::{ClaimWhen, Lever, MechanismClaim, Order, Positions, Surface};
    use crate::domain::mechanism::Mechanism;
    use crate::domain::name::WindowId;
    use crate::domain::testkit::{self, Gen, Sample};

    fn trial(lever: Lever) -> (Experiment, Window) {
        let spec = ExperimentSpec::new(testkit::spec_parts(lever)).unwrap();
        let evidence = evidence_class(spec.hypothesis(), &Default::default());
        let experiment = Experiment::new(spec, evidence);
        let window = Window::trial(WindowId::new(1), &experiment);
        (experiment, window)
    }

    fn behavioral_lever() -> Lever {
        Lever::new(
            "effort".parse().unwrap(),
            Positions::new(
                Order::Ordinal,
                vec!["low".parse().unwrap(), "high".parse().unwrap()],
            )
            .unwrap(),
            Surface::ProjectSettings,
            RoleScope::All,
            vec![MechanismClaim {
                mechanism: Mechanism::ReasoningVolume,
                reduced_when: ClaimWhen::Down,
            }],
        )
        .unwrap()
    }

    fn draw(
        window: &Window,
        n: usize,
        rng: &mut Gen,
        sample: impl Fn(&mut Gen, Arm) -> Sample,
    ) -> Vec<ObservedRun> {
        (0..n)
            .map(|_| {
                let arm = if rng.unit() < window.p() {
                    Arm::Treatment
                } else {
                    Arm::Control
                };
                let s = sample(rng, arm);
                testkit::observed(window, arm, s)
            })
            .collect()
    }

    #[test]
    fn too_few_runs_is_pending() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let mut rng = Gen::new(1);
        let runs = draw(&window, 6, &mut rng, |_, _| Sample::new(100, 100, true));
        assert_eq!(evaluate(&experiment, &window, &runs), Verdict::Pending);
    }

    #[test]
    fn a_priced_saving_that_shows_in_the_bill_is_kept_provisionally() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let mut rng = Gen::new(2);
        let runs = draw(&window, 600, &mut rng, |g, arm| {
            let write = if arm == Arm::Treatment { 400 } else { 1000 };
            Sample::new(g.around(write, 50), g.around(500, 50), true)
        });
        let verdict = evaluate(&experiment, &window, &runs);
        assert!(
            matches!(verdict, Verdict::ProvisionalKeep(_)),
            "{verdict:?}"
        );
    }

    #[test]
    fn a_saving_the_bill_does_not_show_is_not_kept() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let mut rng = Gen::new(3);
        let runs = draw(&window, 600, &mut rng, |g, arm| {
            let (write, read) = if arm == Arm::Treatment {
                (400, 1400)
            } else {
                (1000, 500)
            };
            Sample::new(g.around(write, 50), g.around(read, 50), true)
        });
        let verdict = evaluate(&experiment, &window, &runs);
        assert!(
            matches!(
                verdict,
                Verdict::Revert(RevertReason::Shifted | RevertReason::Worse, _)
            ),
            "{verdict:?}"
        );
    }

    #[test]
    fn terms_that_fall_while_the_bill_holds_are_shifted() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let mut rng = Gen::new(13);
        // writes fall by 600, reads rise by 650: the bill is a little higher,
        // not clearly worse, but the net of the hypothesis is clearly up
        let runs = draw(&window, 2000, &mut rng, |g, arm| {
            let (write, read) = if arm == Arm::Treatment {
                (400, 1150)
            } else {
                (1000, 500)
            };
            Sample::new(g.around(write, 20), g.around(read, 20), true)
        });
        let verdict = evaluate(&experiment, &window, &runs);
        assert!(
            matches!(
                verdict,
                Verdict::Revert(RevertReason::Shifted | RevertReason::Worse, _)
            ),
            "{verdict:?}"
        );
    }

    #[test]
    fn a_behavioural_lever_is_kept_on_the_objective() {
        let (experiment, window) = trial(behavioral_lever());
        assert_eq!(experiment.evidence(), EvidenceClass::Behavioral);
        let mut rng = Gen::new(4);
        let runs = draw(&window, 600, &mut rng, |g, arm| {
            let read = if arm == Arm::Treatment { 300 } else { 1000 };
            Sample::new(g.around(200, 20), g.around(read, 50), true)
        });
        let verdict = evaluate(&experiment, &window, &runs);
        assert!(matches!(verdict, Verdict::Keep(_)), "{verdict:?}");
    }

    #[test]
    fn a_falling_success_rate_breaches_its_guardrail() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let mut rng = Gen::new(14);
        let runs = draw(&window, 1500, &mut rng, |g, arm| {
            let rate = if arm == Arm::Treatment { 0.5 } else { 0.95 };
            let write = if arm == Arm::Treatment { 400 } else { 1000 };
            Sample::new(g.around(write, 50), 500, g.unit() < rate)
        });
        let verdict = evaluate(&experiment, &window, &runs);
        assert!(
            matches!(
                verdict,
                Verdict::Revert(RevertReason::GuardrailBreached(Guardrail::SuccessRate), _)
            ),
            "{verdict:?}"
        );
    }

    #[test]
    fn failures_in_a_row_trip_before_anything_else() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let mut rng = Gen::new(5);
        let runs = draw(&window, 100, &mut rng, |_, arm| {
            Sample::new(100, 100, arm == Arm::Control)
        });
        assert_eq!(evaluate(&experiment, &window, &runs), Verdict::Tripped);
    }

    #[test]
    fn a_broken_split_halts() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let mut rng = Gen::new(6);
        let runs: Vec<_> = (0..400)
            .map(|i| {
                let arm = if i % 10 == 0 {
                    Arm::Control
                } else {
                    Arm::Treatment
                };
                testkit::observed(&window, arm, Sample::new(rng.around(100, 5), 100, true))
            })
            .collect();
        assert_eq!(
            evaluate(&experiment, &window, &runs),
            Verdict::Halted(TrustBreach::ArmImbalance)
        );
    }

    #[test]
    fn an_undecided_trial_is_reverted_at_its_maximum() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let mut rng = Gen::new(7);
        let max = experiment.spec().stopping().max_runs() as usize;
        let runs = draw(&window, max, &mut rng, |g, _| {
            Sample::new(g.around(1000, 900), 500, true)
        });
        let verdict = evaluate(&experiment, &window, &runs);
        assert!(
            matches!(
                verdict,
                Verdict::Revert(RevertReason::Exhausted | RevertReason::NoEffect, _)
            ),
            "{verdict:?}"
        );
    }

    #[test]
    fn runs_of_other_windows_do_not_count() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let other = Window::trial(WindowId::new(9), &experiment);
        let mut rng = Gen::new(8);
        let runs = draw(&other, 600, &mut rng, |g, arm| {
            let write = if arm == Arm::Treatment { 400 } else { 1000 };
            Sample::new(g.around(write, 50), 500, true)
        });
        assert_eq!(evaluate(&experiment, &window, &runs), Verdict::Pending);
    }

    #[test]
    fn peeking_at_the_boundary_rarely_keeps() {
        // the treatment sits exactly at (1 − δ) of the control: any keep is false
        let (experiment, window) = trial(testkit::ttl_lever());
        let delta = experiment.spec().threshold();
        let mut false_keeps = 0;
        for seed in 0..60 {
            let mut rng = Gen::new(1000 + seed);
            let mut seen = Vec::new();
            for _ in 0..300 {
                let arm = if rng.unit() < window.p() {
                    Arm::Treatment
                } else {
                    Arm::Control
                };
                let base = 1000.0
                    * if arm == Arm::Treatment {
                        1.0 - delta
                    } else {
                        1.0
                    };
                let write = rng.lognormal(base, 0.5);
                seen.push(testkit::observed(
                    &window,
                    arm,
                    Sample::new(write, 500, true),
                ));
                if let Verdict::ProvisionalKeep(_) | Verdict::Keep(_) =
                    evaluate(&experiment, &window, &seen)
                {
                    false_keeps += 1;
                    break;
                }
            }
        }
        // α_primary is 0.05 in the test spec: 60 sequences expect about 3
        assert!(false_keeps <= 8, "{false_keeps} false keeps of 60");
    }

    #[test]
    fn a_kept_lever_shown_worse_than_the_holdout_is_worse() {
        let (experiment, _) = trial(testkit::ttl_lever());
        let window = Window::watch(WindowId::new(2), &experiment);
        let mut rng = Gen::new(9);
        let runs = draw(&window, 800, &mut rng, |g, arm| {
            let write = if arm == Arm::Treatment { 1500 } else { 1000 };
            Sample::new(g.around(write, 50), 500, true)
        });
        let spec = experiment.spec();
        let verdict = watch(&window, spec.stopping(), spec.trust(), &runs);
        assert!(matches!(verdict, WatchVerdict::Worse(_)), "{verdict:?}");
    }

    #[test]
    fn a_confirmation_keeps_a_saving_the_bill_shows() {
        let (experiment, _) = trial(testkit::ttl_lever());
        let window = Window::confirm(WindowId::new(3), &experiment);
        let mut rng = Gen::new(10);
        let runs = draw(&window, 1500, &mut rng, |g, arm| {
            let write = if arm == Arm::Treatment { 400 } else { 1000 };
            Sample::new(g.around(write, 50), g.around(500, 50), true)
        });
        let verdict = confirm(&experiment, &window, &runs);
        assert!(matches!(verdict, Confirmation::Keep(_)), "{verdict:?}");
    }

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
}
