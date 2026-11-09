//! The Lab's verdicts: what a window of runs proves about a step.
//!
//! ```text
//! evaluate: a trial                                 ──> Verdict
//! confirm:  a provisional keep against the holdout  ──> Confirmation
//! watch:    a kept lever against the holdout        ──> WatchVerdict
//! ```
//!
//! All three read a window through [`crate::domain::measure`]. Inside
//! `evaluate` the order is: the gate (a run of failures, a broken split, too
//! few runs, a broken trust check), then a guardrail clearly worse, then the
//! objective clearly worse, then cost terms that fell while the bill did not
//! (shifted), then the threshold cleared with every guardrail shown not worse,
//! then no effect shown, then the maximum, else pending. A failure streak comes
//! first because it harms the user whatever its cause.

use crate::domain::assign::{ObservedRun, Window};
use crate::domain::cost::TermSet;
use crate::domain::lab::experiment::Experiment;
use crate::domain::measure::{
    ArmCounts, Gate, Guardrail, GuardrailCheck, GuardrailIntervals, Look, Measure, ObjectiveCall,
    RelativeChange, Row, StoppingRule, TrustBreach, TrustLimits, counts, eligible, gate,
    guardrails, means, objective_call, ratio_diff, relative, sum_of,
};
use crate::domain::mechanism::EvidenceClass;
use crate::domain::stats::Interval;

/// The evidence a trial was judged on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EvidenceDetail {
    /// The cost terms of the hypothesis.
    Priced {
        /// `(1−δ)·R_control − R_treatment` of the terms that should fall, per
        /// unit of the terms the step leaves alone.
        lowers: Interval,
        /// `R_control − R_treatment` of those and the terms that may rise.
        net: Interval,
    },
    /// The cost per successful run directly.
    Behavioral,
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

const MEASURES: [Measure; 3] = [Measure::Objective, Measure::Duration, Measure::Terms];

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
            // the cells the step does not move measure the size of a run, so
            // the terms are compared per unit of it, not as raw amounts
            let untouched = TermSet::all().difference(&net_set);
            let size = sum_of(&untouched);
            let lowers = means(
                &terms,
                window.p(),
                1.0 - delta,
                1.0,
                &sum_of(h.lowers()),
                &size,
                look,
            )
            .interval;
            let net = means(&terms, window.p(), 1.0, 1.0, &sum_of(&net_set), &size, look).interval;
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
    // the gate let the window through only to say it cannot be decided
    if gate == Gate::Exhausted {
        return Verdict::Revert(RevertReason::Exhausted, response);
    }
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
    if at_max {
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
    if gate == Gate::Exhausted {
        return Confirmation::Revert(RevertReason::Exhausted, response);
    }
    if let GuardrailCheck::Breached(g, _) = guard {
        return Confirmation::Revert(RevertReason::GuardrailBreached(g), response);
    }
    match objective_call(&objective.interval, &clears) {
        ObjectiveCall::Worse => Confirmation::Revert(RevertReason::Worse, response),
        ObjectiveCall::Cleared if matches!(guard, GuardrailCheck::NonInferior(_)) => {
            Confirmation::Keep(response)
        }
        ObjectiveCall::NoEffect => Confirmation::Revert(RevertReason::NoEffect, response),
        _ if all.len() as u32 >= max => Confirmation::Revert(RevertReason::Exhausted, response),
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
    let all: Vec<&Row> = rows.of(Measure::Objective);
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
    use crate::domain::assign::Arm;
    use crate::domain::lab::experiment::{
        ExperimentSpec, confirm_window, evidence_class, trial_window, watch_window,
    };
    use crate::domain::lever::{
        ClaimWhen, Lever, MechanismClaim, Order, Positions, RoleScope, Surface,
    };
    use crate::domain::mechanism::Mechanism;
    use crate::domain::name::WindowId;
    use crate::domain::testkit::{self, Gen, Sample};

    fn trial(lever: Lever) -> (Experiment, Window) {
        let spec = ExperimentSpec::new(testkit::spec_parts(lever)).unwrap();
        let evidence = evidence_class(spec.hypothesis(), &Default::default());
        let experiment = Experiment::new(spec, evidence);
        let window = trial_window(WindowId::new(1), &experiment);
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
    fn runs_of_other_windows_do_not_count() {
        let (experiment, window) = trial(testkit::ttl_lever());
        let other = trial_window(WindowId::new(9), &experiment);
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
        let window = watch_window(WindowId::new(2), &experiment);
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
        let window = confirm_window(WindowId::new(3), &experiment);
        let mut rng = Gen::new(10);
        let runs = draw(&window, 1500, &mut rng, |g, arm| {
            let write = if arm == Arm::Treatment { 400 } else { 1000 };
            Sample::new(g.around(write, 50), g.around(500, 50), true)
        });
        let verdict = confirm(&experiment, &window, &runs);
        assert!(matches!(verdict, Confirmation::Keep(_)), "{verdict:?}");
    }
}
