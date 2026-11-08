//! Whether an adopted technique fits a customer, and what is seen so far.
//!
//! The catalogue already says what the technique does; what is checked here
//! is that it does not make this customer's runs worse. The technique fits
//! once its window shows the cost per successful run no worse than the
//! catalogue's margin and every guardrail no worse than its own, and does not
//! fit as soon as either is shown worse. The comparison is the shared one of
//! [`crate::domain::measure`], with the limits the catalogue fixed.
//!
//! What is compared depends on the technique's mechanisms. A technique that
//! only changes prices ([`crate::domain::mechanism::evidence_of`]) is compared
//! on the cost terms its mechanisms move, per unit of the terms it leaves
//! alone: those do not depend on how the agent works, and the size of a run
//! does not drown them; its guardrails need only not be shown worse, since it
//! does not change how the agent works. Any other technique is compared on
//! the cost per successful run, and must show its guardrails not worse.
//!
//! A [`Reading`] is always available, before any call: what the catalogue
//! expects, and the treatment's measure against the control's so far.

use crate::domain::assign::{ObservedRun, Window};
use crate::domain::cost::TermSet;
use crate::domain::measure::{
    ArmCounts, Eligible, Estimate, Gate, GuardrailCheck, Look, Measure, RelativeChange, Row,
    counts, eligible, gate, guardrails, means, ratio_diff, relative, sum_of,
};
use crate::domain::mechanism::{EvidenceClass, effect_of, evidence_of};
use crate::domain::name::TechniqueId;
use crate::domain::product::adoption::{Adoption, AdoptionState, UnfitReason};
use crate::domain::product::catalogue::{Expected, Technique};
use crate::domain::stats::Interval;

/// What a fit is compared on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The cost terms the technique moves, per unit of those it leaves alone.
    CostTerms,
    /// The cost per successful run.
    Objective,
}

fn basis(technique: &Technique) -> Basis {
    let priced = technique
        .mechanisms()
        .iter()
        .all(|m| evidence_of(*m) == EvidenceClass::Priced);
    if priced {
        Basis::CostTerms
    } else {
        Basis::Objective
    }
}

/// The cells a technique's mechanisms move, either way.
fn moved(technique: &Technique) -> TermSet {
    technique
        .mechanisms()
        .iter()
        .map(|m| effect_of(*m))
        .fold(TermSet::empty(), |set, effect| {
            set.union(effect.lowers()).union(effect.may_raise())
        })
}

/// `a·R_control − b·R_treatment` of the technique's basis.
fn compare(
    technique: &Technique,
    window: &Window,
    rows: &Eligible,
    a: f64,
    b: f64,
    look: Look,
) -> (Estimate, usize) {
    match basis(technique) {
        Basis::Objective => {
            let all: Vec<&Row> = rows.of(Measure::Objective);
            (ratio_diff(window, &all, a, b, look), all.len())
        }
        Basis::CostTerms => {
            let terms: Vec<&Row> = rows.of(Measure::Terms);
            let set = moved(technique);
            let untouched = TermSet::all().difference(&set);
            let estimate = means(
                &terms,
                window.p(),
                a,
                b,
                &sum_of(&set),
                &sum_of(&untouched),
                look,
            );
            (estimate, terms.len())
        }
    }
}

/// What a window says about an adopted technique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitCall {
    /// More runs are needed.
    Pending,
    /// Shown no worse than the margins.
    Fits,
    /// Shown worse, or stopped.
    Unfit(UnfitReason),
    /// The maximum was reached without a call.
    Undecided,
}

/// What is expected and what is seen so far, for a person to read.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// The technique.
    pub technique: TechniqueId,
    /// Where the adoption is.
    pub state: AdoptionState,
    /// What the catalogue expects, on the cost per successful run.
    pub expected: Expected,
    /// What the fit is compared on.
    pub basis: Basis,
    /// The treatment's measure against the control's so far, once both arms
    /// have one.
    pub observed: Option<RelativeChange>,
    /// `R_control − R_treatment` of the measure so far.
    pub interval: Interval,
    /// The runs counted, per arm.
    pub counts: ArmCounts,
}

const MEASURES: [Measure; 3] = [Measure::Objective, Measure::Duration, Measure::Terms];

/// What the adoption's window says now, with the reading that goes with it.
pub fn fit(adoption: &Adoption, runs: &[ObservedRun]) -> (FitCall, Reading) {
    let technique = adoption.technique();
    let mut reading = Reading {
        technique: technique.id().clone(),
        state: adoption.state(),
        expected: technique.expected(),
        basis: basis(technique),
        observed: None,
        interval: Interval::unbounded(),
        counts: ArmCounts::default(),
    };
    let Some(window) = adoption.window() else {
        return (FitCall::Pending, reading);
    };
    let spec = technique.fit();
    let rows = eligible(window, runs);
    let look = Look::new(spec.stopping.rho(), spec.stopping.alpha().primary());
    let (seen, _) = compare(technique, window, &rows, 1.0, 1.0, look);
    reading.counts = counts(&rows.of(Measure::Objective));
    reading.interval = seen.interval;
    if seen.control > 0.0 && seen.treatment > 0.0 {
        reading.observed = Some(relative(&seen));
    }
    let max = spec.stopping.max_runs();
    let call = match gate(
        window,
        &rows,
        &MEASURES,
        &spec.stopping,
        &spec.trust,
        Some(max),
    ) {
        Gate::Tripped => FitCall::Unfit(UnfitReason::Tripped),
        Gate::Halted(breach) => FitCall::Unfit(UnfitReason::Halted(breach)),
        Gate::Pending => FitCall::Pending,
        Gate::Exhausted => FitCall::Undecided,
        Gate::Proceed => judge(window, &rows, adoption, look),
    };
    (call, reading)
}

fn judge(window: &Window, rows: &Eligible, adoption: &Adoption, look: Look) -> FitCall {
    let technique = adoption.technique();
    let spec = technique.fit();
    let guard = guardrails(window, rows, &spec.stopping, &spec.margins);
    if let GuardrailCheck::Breached(g, _) = guard {
        return FitCall::Unfit(UnfitReason::GuardrailBreached(g));
    }
    // (1 + margin)·R_control − R_treatment: below zero is worse than allowed
    let (allowed, n) = compare(technique, window, rows, 1.0 + spec.margin.get(), 1.0, look);
    if allowed.interval.upper() < 0.0 {
        return FitCall::Unfit(UnfitReason::Worse);
    }
    // a technique that only changes prices does not change how the agent
    // works, so its guardrails need only not be shown worse; one that changes
    // behaviour must show them not worse
    let guarded = match basis(technique) {
        Basis::CostTerms => true,
        Basis::Objective => matches!(guard, GuardrailCheck::NonInferior(_)),
    };
    if allowed.interval.lower() > 0.0 && guarded {
        return FitCall::Fits;
    }
    if n as u32 >= spec.stopping.max_runs() {
        FitCall::Undecided
    } else {
        FitCall::Pending
    }
}
