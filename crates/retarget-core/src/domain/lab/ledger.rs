//! The Lab's ledger of a scope: its experiment, its watches, what it gave up
//! on, and what it still has to put back.
//!
//! Every decision is taken in [`Ledger::decide`], which returns the events that
//! record it, derived events and window ids included. [`Ledger::apply`] folds
//! an event into the state and decides nothing, so a ledger rebuilt from its
//! events ([`Ledger::replay`]) is the same ledger. Writing and putting back go
//! through [`crate::domain::places`].
//!
//! ```text
//! command ──> decide (reads the state, calls the verdicts) ──> events ──> apply ──> state
//! ```
//!
//! # Invariants
//!
//! - One experiment at a time per scope. A watched lever, a lever with a change
//!   still to put back, a retired lever, a lever written only in the user's
//!   scope, and a step given up on are not started.
//! - An event carries the inputs of a decision; folding it checks them again
//!   (an experiment's evidence is recomputed and must match), so a stored log
//!   cannot smuggle in a decision the domain would not take.
//! - A change is put back to the position the experiment started from.
//! - Only a newer harness version counts as a change of version.
//!
//! # Errors
//!
//! [`LedgerError`] says which rule a command broke. A command refused leaves
//! the ledger unchanged; send the command the rule asks for first (close the
//! watch, put the change back, resolve the edit), or none.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::assign::{ObservedRun, Window, WindowKind};
use crate::domain::lab::experiment::{
    Experiment, ExperimentError, ExperimentSpec, confirm_window, evidence_class, trial_window,
    watch_window,
};
use crate::domain::lab::verdict::{
    Confirmation, RevertReason, Verdict, WatchVerdict, confirm, evaluate, watch,
};
use crate::domain::lever::{Applicability, Surface};
use crate::domain::measure::TrustBreach;
use crate::domain::mechanism::{EvidenceClass, Mechanism};
use crate::domain::name::{
    Digest, ExperimentId, HarnessVersion, LeverKey, PositionId, Scope, WindowId,
};
use crate::domain::places::Places;
use crate::domain::put_back::{Baseline, PutBack};

/// What must happen before a step given up on may be tried again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryCondition {
    /// A newer harness version.
    HarnessChanged,
    /// Another lever acting on this mechanism was kept.
    MechanismKept(Mechanism),
}

/// A step tried and put back, with when it may be tried again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GivenUp {
    /// The lever.
    pub lever: LeverKey,
    /// The position it was moved to.
    pub position: PositionId,
    /// Any of these reopens it.
    pub conditions: Vec<RetryCondition>,
}

/// How a mechanism's cost terms have foretold the objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SurrogateRecord {
    /// Confirmations that kept what the terms promised.
    pub hits: u32,
    /// Confirmations that showed the terms wrong.
    pub misses: u32,
}

/// When a mechanism's terms stop being trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurrogatePolicy {
    /// The misses at least needed, and more than the hits.
    pub min_misses: u32,
}

impl SurrogateRecord {
    fn demoted(&self, policy: &SurrogatePolicy) -> bool {
        self.misses >= policy.min_misses && self.misses > self.hits
    }
}

/// The experiment of a scope and the window it is judged on now.
#[derive(Debug, Clone, PartialEq)]
pub struct Active {
    experiment: Experiment,
    window: Window,
}

impl Active {
    /// The experiment.
    pub fn experiment(&self) -> &Experiment {
        &self.experiment
    }
    /// The window of runs it is judged on now (its trial, or its confirmation).
    pub fn window(&self) -> &Window {
        &self.window
    }
}

/// A kept lever, still compared with a small holdout.
#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    experiment: Experiment,
    window: Window,
}

impl Watch {
    /// The experiment that kept it.
    pub fn experiment(&self) -> &Experiment {
        &self.experiment
    }
    /// The window of runs it is watched on now.
    pub fn window(&self) -> &Window {
        &self.window
    }
}

/// Something the ledger decided or recorded.
#[derive(Debug, Clone, PartialEq)]
pub enum LedgerEvent {
    /// An experiment started, with its first window and what its lever's place held.
    ExperimentStarted {
        /// Its spec.
        spec: Box<ExperimentSpec>,
        /// The evidence it is judged on.
        evidence: EvidenceClass,
        /// Its trial window.
        window: Window,
        /// What its lever's place held.
        baseline: Baseline,
    },
    /// A window replaced the active experiment's or a watch's.
    WindowReopened(Window),
    /// A run was recorded.
    RunRecorded(ObservedRun),
    /// A newer harness version was seen.
    HarnessChanged(HarnessVersion),
    /// A trial reached a verdict.
    Decided(ExperimentId, Verdict),
    /// A confirmation reached a verdict.
    Confirmed(ExperimentId, Confirmation),
    /// A watch reached a verdict.
    WatchDecided(LeverKey, WatchVerdict),
    /// The active experiment became a watch.
    WatchOpened(ExperimentId, Window),
    /// The active experiment ended.
    ActiveClosed,
    /// A watch ended.
    WatchClosed(LeverKey),
    /// A step was given up on.
    GivenUpAdded(GivenUp),
    /// A step given up on may be tried again.
    RetryReopened(LeverKey, PositionId),
    /// A mechanism's terms foretold the objective.
    SurrogateHit(Mechanism),
    /// A mechanism's terms did not.
    SurrogateMiss(Mechanism),
    /// A write of a lever's place may go ahead.
    WriteApproved(LeverKey, Option<Digest>),
    /// A lever's place was written.
    Written(LeverKey, Digest),
    /// Someone edited a lever's place; it is blocked at this position.
    ExternalEditFound(LeverKey, PositionId),
    /// A change has to be put back to this position.
    PutBackDue(LeverKey, PositionId),
    /// Putting a change back may go ahead.
    PutBackApproved(LeverKey),
    /// A change was put back.
    PutBackDone(LeverKey),
    /// A person accepted their edit as the lever's place.
    ExternalEditResolved(LeverKey),
    /// A lever was retired.
    LeverRetired(LeverKey),
}

/// What the ledger is asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Start an experiment; `current` is what its lever's place holds now.
    Start(Box<ExperimentSpec>, Option<Digest>),
    /// Record a finished run.
    Record(Box<ObservedRun>),
    /// Judge the active experiment and every watch.
    Evaluate,
    /// About to write a lever's place for a run; `current` is what it holds.
    PrepareWrite(LeverKey, Option<Digest>),
    /// A lever's place was written.
    Written(LeverKey, Digest),
    /// About to put a change back; `current` is what the place holds.
    CheckPutBack(LeverKey, Option<Digest>),
    /// A change was put back.
    PutBackWritten(LeverKey),
    /// A person accepts their edit.
    ResolveExternalEdit(LeverKey),
    /// Stop watching a kept lever.
    CloseWatch(LeverKey),
    /// Never propose a lever again.
    Retire(LeverKey),
}

/// Why a command was refused, or an event could not be folded.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LedgerError {
    /// An experiment is already running in this scope.
    #[error("an experiment is already running in this scope")]
    ActiveExists,
    /// The lever is retired.
    #[error("lever `{0}` is retired")]
    Retired(LeverKey),
    /// The step was given up on and nothing has reopened it.
    #[error("this step of lever `{0}` was given up on")]
    GivenUp(LeverKey),
    /// The lever is written in a surface retarget only shows.
    #[error("retarget does not write {0:?}")]
    NotWritable(Surface),
    /// The lever is being watched; close the watch first.
    #[error("lever `{0}` is being watched")]
    Watched(LeverKey),
    /// The lever is not being watched.
    #[error("lever `{0}` is not being watched")]
    NotWatched(LeverKey),
    /// The lever is in use by the experiment or a watch.
    #[error("lever `{0}` is in use")]
    InUse(LeverKey),
    /// The lever still has a change to put back.
    #[error("lever `{0}` still has a change to put back")]
    PutBackPending(LeverKey),
    /// Neither the experiment nor a watch writes this lever.
    #[error("nothing writes lever `{0}`")]
    NoActive(LeverKey),
    /// The lever has nothing to put back.
    #[error("lever `{0}` has nothing to put back")]
    NothingToPutBack(LeverKey),
    /// The write was not approved first.
    #[error("the write of lever `{0}` was not approved")]
    NotApproved(LeverKey),
    /// The lever is not blocked by an edit.
    #[error("lever `{0}` is not blocked")]
    NotBlocked(LeverKey),
    /// An event does not fit the state it is folded into.
    #[error("an event does not fit the ledger: {0}")]
    UnfoldableEvent(&'static str),
    /// A spec was refused.
    #[error(transparent)]
    Experiment(#[from] ExperimentError),
}

/// The Lab's ledger of one scope.
#[derive(Debug, Clone, PartialEq)]
pub struct Ledger {
    scope: Scope,
    policy: SurrogatePolicy,
    harness: Option<HarnessVersion>,
    next_window: u64,
    active: Option<Active>,
    watches: BTreeMap<LeverKey, Watch>,
    places: Places,
    given_up: Vec<GivenUp>,
    retired: BTreeSet<LeverKey>,
    surrogates: BTreeMap<Mechanism, SurrogateRecord>,
    runs: Vec<ObservedRun>,
}

impl Ledger {
    /// An empty ledger.
    pub fn new(scope: Scope, policy: SurrogatePolicy) -> Self {
        Self {
            scope,
            policy,
            harness: None,
            next_window: 1,
            active: None,
            watches: BTreeMap::new(),
            places: Places::default(),
            given_up: Vec::new(),
            retired: BTreeSet::new(),
            surrogates: BTreeMap::new(),
            runs: Vec::new(),
        }
    }

    /// A ledger rebuilt from its events.
    pub fn replay(
        scope: Scope,
        policy: SurrogatePolicy,
        events: &[LedgerEvent],
    ) -> Result<Self, LedgerError> {
        let mut ledger = Self::new(scope, policy);
        for event in events {
            ledger.apply(event)?;
        }
        Ok(ledger)
    }

    /// Decide a command and fold its events; the events are returned for the store.
    pub fn execute(&mut self, command: Command) -> Result<Vec<LedgerEvent>, LedgerError> {
        let events = self.decide(command)?;
        for event in &events {
            self.apply(event)?;
        }
        Ok(events)
    }

    /// The scope.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    /// The active experiment.
    pub fn active(&self) -> Option<&Active> {
        self.active.as_ref()
    }
    /// The watches, by lever.
    pub fn watches(&self) -> &BTreeMap<LeverKey, Watch> {
        &self.watches
    }
    /// The windows runs are assigned in now.
    pub fn windows(&self) -> Vec<&Window> {
        self.active
            .iter()
            .map(|a| &a.window)
            .chain(self.watches.values().map(|w| &w.window))
            .collect()
    }
    /// The places of the levers, and what must be put back.
    pub fn places(&self) -> &Places {
        &self.places
    }
    /// The changes to put back, and where to.
    pub fn pending_put_back(&self) -> Vec<(&LeverKey, &PositionId)> {
        self.places.pending_put_back()
    }
    /// The changes left as a person edited them.
    pub fn blocked(&self) -> Vec<&LeverKey> {
        self.places.blocked()
    }
    /// The steps given up on.
    pub fn given_up(&self) -> &[GivenUp] {
        &self.given_up
    }

    fn demoted(&self) -> BTreeSet<Mechanism> {
        self.surrogates
            .iter()
            .filter(|(_, record)| record.demoted(&self.policy))
            .map(|(m, _)| *m)
            .collect()
    }

    /// Decide a command: the events that record it, or why it is refused.
    pub fn decide(&self, command: Command) -> Result<Vec<LedgerEvent>, LedgerError> {
        let mut ids = self.next_window;
        let mut next_id = move || {
            let id = WindowId::new(ids);
            ids += 1;
            id
        };
        match command {
            Command::Start(spec, current) => {
                let lever = spec.lever().key().clone();
                if self.active.is_some() {
                    return Err(LedgerError::ActiveExists);
                }
                if self.retired.contains(&lever) {
                    return Err(LedgerError::Retired(lever));
                }
                if spec.lever().surface().applicability() == Applicability::ShowOnly {
                    return Err(LedgerError::NotWritable(spec.lever().surface()));
                }
                if self.watches.contains_key(&lever) {
                    return Err(LedgerError::Watched(lever));
                }
                if self.places.is_pending(&lever) {
                    return Err(LedgerError::PutBackPending(lever));
                }
                if self
                    .given_up
                    .iter()
                    .any(|g| g.lever == lever && &g.position == spec.to())
                {
                    return Err(LedgerError::GivenUp(lever));
                }
                let evidence = evidence_class(spec.hypothesis(), &self.demoted());
                let experiment = Experiment::new(spec.as_ref().clone(), evidence);
                let window = trial_window(next_id(), &experiment);
                let baseline = current.map_or(Baseline::Empty, Baseline::Held);
                Ok(vec![LedgerEvent::ExperimentStarted {
                    spec,
                    evidence,
                    window,
                    baseline,
                }])
            }
            Command::Record(run) => {
                let version = run.run.start().harness;
                let mut events = vec![LedgerEvent::RunRecorded(*run)];
                match self.harness {
                    None => events.push(LedgerEvent::HarnessChanged(version)),
                    Some(known) if version > known => {
                        events.push(LedgerEvent::HarnessChanged(version));
                        for g in &self.given_up {
                            if g.conditions.contains(&RetryCondition::HarnessChanged) {
                                events.push(LedgerEvent::RetryReopened(
                                    g.lever.clone(),
                                    g.position.clone(),
                                ));
                            }
                        }
                        for watch in self.watches.values() {
                            events.push(LedgerEvent::WindowReopened(watch_window(
                                next_id(),
                                &watch.experiment,
                            )));
                        }
                    }
                    Some(_) => {}
                }
                Ok(events)
            }
            Command::Evaluate => {
                let mut events = Vec::new();
                if let Some(active) = &self.active {
                    self.decide_active(active, &mut next_id, &mut events);
                }
                for (lever, w) in &self.watches {
                    let spec = w.experiment.spec();
                    let verdict = watch(&w.window, spec.stopping(), spec.trust(), &self.runs);
                    self.after_watch(lever, &w.experiment, verdict, &mut events);
                }
                Ok(events)
            }
            Command::PrepareWrite(lever, current) => {
                let (kind, experiment) = self
                    .active
                    .as_ref()
                    .filter(|a| a.window.lever() == &lever)
                    .map(|a| (a.window.kind().clone(), a.experiment.clone()))
                    .or_else(|| {
                        self.watches
                            .get(&lever)
                            .map(|w| (w.window.kind().clone(), w.experiment.clone()))
                    })
                    .ok_or_else(|| LedgerError::NoActive(lever.clone()))?;
                match self.places.check(&lever, current.as_ref()) {
                    PutBack::Proceed => Ok(vec![LedgerEvent::WriteApproved(lever, current)]),
                    PutBack::ExternalEdit => {
                        let from = experiment.spec().from().clone();
                        let halted = TrustBreach::ExternalEdit;
                        let id = experiment.spec().id();
                        let mut events = match kind {
                            WindowKind::Confirm(_) => vec![
                                LedgerEvent::Confirmed(id, Confirmation::Halted(halted)),
                                LedgerEvent::ActiveClosed,
                            ],
                            WindowKind::Watch(_) => vec![
                                LedgerEvent::WatchDecided(
                                    lever.clone(),
                                    WatchVerdict::Halted(halted),
                                ),
                                LedgerEvent::WatchClosed(lever.clone()),
                            ],
                            WindowKind::Trial(_) | WindowKind::Adoption(_) => vec![
                                LedgerEvent::Decided(id, Verdict::Halted(halted)),
                                LedgerEvent::ActiveClosed,
                            ],
                        };
                        events.push(LedgerEvent::ExternalEditFound(lever, from));
                        Ok(events)
                    }
                }
            }
            Command::Written(lever, digest) => {
                if self.places.write_approved(&lever) {
                    Ok(vec![LedgerEvent::Written(lever, digest)])
                } else {
                    Err(LedgerError::NotApproved(lever))
                }
            }
            Command::CheckPutBack(lever, current) => {
                let to = self
                    .places
                    .due(&lever)
                    .cloned()
                    .ok_or_else(|| LedgerError::NothingToPutBack(lever.clone()))?;
                Ok(match self.places.check(&lever, current.as_ref()) {
                    PutBack::Proceed => vec![LedgerEvent::PutBackApproved(lever)],
                    PutBack::ExternalEdit => vec![LedgerEvent::ExternalEditFound(lever, to)],
                })
            }
            Command::PutBackWritten(lever) => {
                if self.places.put_back_approved(&lever) {
                    Ok(vec![LedgerEvent::PutBackDone(lever)])
                } else {
                    Err(LedgerError::NotApproved(lever))
                }
            }
            Command::ResolveExternalEdit(lever) => {
                if self.places.is_blocked(&lever) {
                    Ok(vec![LedgerEvent::ExternalEditResolved(lever)])
                } else {
                    Err(LedgerError::NotBlocked(lever))
                }
            }
            Command::CloseWatch(lever) => {
                if self.watches.contains_key(&lever) {
                    Ok(vec![LedgerEvent::WatchClosed(lever)])
                } else {
                    Err(LedgerError::NotWatched(lever))
                }
            }
            Command::Retire(lever) => {
                let active = self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.window.lever() == &lever);
                if active || self.watches.contains_key(&lever) {
                    Err(LedgerError::InUse(lever))
                } else {
                    Ok(vec![LedgerEvent::LeverRetired(lever)])
                }
            }
        }
    }

    fn given_up_for(experiment: &Experiment) -> GivenUp {
        let spec = experiment.spec();
        let mut conditions = vec![RetryCondition::HarnessChanged];
        conditions.extend(
            spec.hypothesis()
                .mechanisms()
                .iter()
                .map(|m| RetryCondition::MechanismKept(*m)),
        );
        GivenUp {
            lever: spec.lever().key().clone(),
            position: spec.to().clone(),
            conditions,
        }
    }

    fn put_back_if_written(&self, experiment: &Experiment, events: &mut Vec<LedgerEvent>) {
        let spec = experiment.spec();
        let lever = spec.lever().key();
        if self.places.wrote(lever) {
            events.push(LedgerEvent::PutBackDue(lever.clone(), spec.from().clone()));
        }
    }

    fn kept(&self, experiment: &Experiment, window: Window, events: &mut Vec<LedgerEvent>) {
        events.push(LedgerEvent::WatchOpened(experiment.spec().id(), window));
        events.push(LedgerEvent::ActiveClosed);
        let mechanisms = experiment.spec().hypothesis().mechanisms();
        for g in &self.given_up {
            if g.conditions
                .iter()
                .any(|c| matches!(c, RetryCondition::MechanismKept(m) if mechanisms.contains(m)))
            {
                events.push(LedgerEvent::RetryReopened(
                    g.lever.clone(),
                    g.position.clone(),
                ));
            }
        }
    }

    fn give_up(&self, experiment: &Experiment, events: &mut Vec<LedgerEvent>) {
        events.push(LedgerEvent::GivenUpAdded(Self::given_up_for(experiment)));
        self.put_back_if_written(experiment, events);
    }

    fn decide_active(
        &self,
        active: &Active,
        next_id: &mut impl FnMut() -> WindowId,
        events: &mut Vec<LedgerEvent>,
    ) {
        let experiment = &active.experiment;
        let id = experiment.spec().id();
        match active.window.kind() {
            WindowKind::Trial(_) => {
                let verdict = evaluate(experiment, &active.window, &self.runs);
                match verdict {
                    Verdict::Pending => {}
                    Verdict::ProvisionalKeep(_) => {
                        events.push(LedgerEvent::Decided(id, verdict));
                        events.push(LedgerEvent::WindowReopened(confirm_window(
                            next_id(),
                            experiment,
                        )));
                    }
                    Verdict::Keep(_) => {
                        events.push(LedgerEvent::Decided(id, verdict));
                        self.kept(experiment, watch_window(next_id(), experiment), events);
                    }
                    Verdict::Revert(..) | Verdict::Tripped => {
                        events.push(LedgerEvent::Decided(id, verdict));
                        events.push(LedgerEvent::ActiveClosed);
                        self.give_up(experiment, events);
                    }
                    Verdict::Halted(_) => {
                        events.push(LedgerEvent::Decided(id, verdict));
                        events.push(LedgerEvent::ActiveClosed);
                        self.put_back_if_written(experiment, events);
                    }
                }
            }
            WindowKind::Confirm(_) => {
                let confirmation = confirm(experiment, &active.window, &self.runs);
                let mechanisms = experiment.spec().hypothesis().mechanisms().clone();
                match confirmation {
                    Confirmation::Pending => {}
                    Confirmation::Keep(_) => {
                        events.push(LedgerEvent::Confirmed(id, confirmation));
                        events.extend(mechanisms.iter().map(|m| LedgerEvent::SurrogateHit(*m)));
                        self.kept(experiment, watch_window(next_id(), experiment), events);
                    }
                    Confirmation::Revert(reason, _) => {
                        events.push(LedgerEvent::Confirmed(id, confirmation));
                        if matches!(reason, RevertReason::Worse | RevertReason::NoEffect) {
                            events
                                .extend(mechanisms.iter().map(|m| LedgerEvent::SurrogateMiss(*m)));
                        }
                        events.push(LedgerEvent::ActiveClosed);
                        self.give_up(experiment, events);
                    }
                    Confirmation::Tripped => {
                        events.push(LedgerEvent::Confirmed(id, confirmation));
                        events.push(LedgerEvent::ActiveClosed);
                        self.give_up(experiment, events);
                    }
                    Confirmation::Halted(_) => {
                        events.push(LedgerEvent::Confirmed(id, confirmation));
                        events.push(LedgerEvent::ActiveClosed);
                        self.put_back_if_written(experiment, events);
                    }
                }
            }
            WindowKind::Watch(_) | WindowKind::Adoption(_) => {}
        }
    }

    fn after_watch(
        &self,
        lever: &LeverKey,
        experiment: &Experiment,
        verdict: WatchVerdict,
        events: &mut Vec<LedgerEvent>,
    ) {
        match verdict {
            WatchVerdict::Holding(_) | WatchVerdict::Pending => {}
            WatchVerdict::Worse(_) | WatchVerdict::Tripped => {
                events.push(LedgerEvent::WatchDecided(lever.clone(), verdict));
                events.push(LedgerEvent::WatchClosed(lever.clone()));
                self.give_up(experiment, events);
            }
            WatchVerdict::Halted(_) => {
                events.push(LedgerEvent::WatchDecided(lever.clone(), verdict));
                events.push(LedgerEvent::WatchClosed(lever.clone()));
                self.put_back_if_written(experiment, events);
            }
        }
    }

    fn bump(&mut self, window: &Window) {
        self.next_window = self.next_window.max(window.id().get() + 1);
    }

    /// Fold an event into the state. Nothing is decided here; an event that
    /// does not fit is refused.
    pub fn apply(&mut self, event: &LedgerEvent) -> Result<(), LedgerError> {
        match event {
            LedgerEvent::ExperimentStarted {
                spec,
                evidence,
                window,
                baseline,
            } => {
                if evidence_class(spec.hypothesis(), &self.demoted()) != *evidence {
                    return Err(LedgerError::UnfoldableEvent(
                        "the evidence does not follow from the ledger",
                    ));
                }
                let experiment = Experiment::new(spec.as_ref().clone(), *evidence);
                if window != &trial_window(window.id(), &experiment) {
                    return Err(LedgerError::UnfoldableEvent(
                        "the window is not the trial's",
                    ));
                }
                self.bump(window);
                self.places.record_baseline(spec.lever().key(), *baseline);
                self.active = Some(Active {
                    experiment,
                    window: window.clone(),
                });
            }
            LedgerEvent::WindowReopened(window) => {
                self.bump(window);
                match window.kind() {
                    WindowKind::Confirm(id) => {
                        let active = self
                            .active
                            .as_mut()
                            .filter(|a| a.experiment.spec().id() == *id)
                            .ok_or(LedgerError::UnfoldableEvent("no experiment to confirm"))?;
                        active.window = window.clone();
                    }
                    WindowKind::Watch(lever) => {
                        let watch = self
                            .watches
                            .get_mut(lever)
                            .ok_or(LedgerError::UnfoldableEvent("no watch to reopen"))?;
                        watch.window = window.clone();
                    }
                    WindowKind::Trial(_) | WindowKind::Adoption(_) => {
                        return Err(LedgerError::UnfoldableEvent(
                            "only confirmations and watches are reopened",
                        ));
                    }
                }
            }
            LedgerEvent::RunRecorded(run) => self.runs.push(run.clone()),
            LedgerEvent::HarnessChanged(version) => self.harness = Some(*version),
            LedgerEvent::Decided(..)
            | LedgerEvent::Confirmed(..)
            | LedgerEvent::WatchDecided(..) => {}
            LedgerEvent::WatchOpened(id, window) => {
                let active = self
                    .active
                    .as_ref()
                    .filter(|a| a.experiment.spec().id() == *id)
                    .ok_or(LedgerError::UnfoldableEvent("no experiment to watch"))?;
                let lever = active.experiment.spec().lever().key().clone();
                let experiment = active.experiment.clone();
                self.bump(window);
                self.watches.insert(
                    lever,
                    Watch {
                        experiment,
                        window: window.clone(),
                    },
                );
            }
            LedgerEvent::ActiveClosed => {
                self.active
                    .take()
                    .ok_or(LedgerError::UnfoldableEvent("no experiment to close"))?;
            }
            LedgerEvent::WatchClosed(lever) => {
                self.watches
                    .remove(lever)
                    .ok_or(LedgerError::UnfoldableEvent("no watch to close"))?;
            }
            LedgerEvent::GivenUpAdded(g) => self.given_up.push(g.clone()),
            LedgerEvent::RetryReopened(lever, position) => self
                .given_up
                .retain(|g| !(&g.lever == lever && &g.position == position)),
            LedgerEvent::SurrogateHit(m) => self.surrogates.entry(*m).or_default().hits += 1,
            LedgerEvent::SurrogateMiss(m) => self.surrogates.entry(*m).or_default().misses += 1,
            LedgerEvent::WriteApproved(lever, current) => {
                self.places.approve_write(lever, *current);
            }
            LedgerEvent::Written(lever, digest) => {
                if !self.places.written(lever, *digest) {
                    return Err(LedgerError::UnfoldableEvent(
                        "a write that was not approved",
                    ));
                }
            }
            LedgerEvent::ExternalEditFound(lever, to) => {
                self.places.external_edit(lever, to.clone());
            }
            LedgerEvent::PutBackDue(lever, to) => {
                self.places.put_back_due(lever, to.clone());
            }
            LedgerEvent::PutBackApproved(lever) => {
                if !self.places.approve_put_back(lever) {
                    return Err(LedgerError::UnfoldableEvent("nothing due to put back"));
                }
            }
            LedgerEvent::PutBackDone(lever) | LedgerEvent::ExternalEditResolved(lever) => {
                self.places.forget(lever);
            }
            LedgerEvent::LeverRetired(lever) => {
                self.retired.insert(lever.clone());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::assign::{Arm, Assigned};
    use crate::domain::testkit::{self, Gen, Sample};

    fn ledger() -> Ledger {
        Ledger::new("repo".parse().unwrap(), SurrogatePolicy { min_misses: 2 })
    }

    fn spec() -> ExperimentSpec {
        ExperimentSpec::new(testkit::spec_parts(testkit::ttl_lever())).unwrap()
    }

    fn digest(b: u8) -> Digest {
        Digest::new([b; 32])
    }

    fn feed(
        ledger: &mut Ledger,
        n: usize,
        rng: &mut Gen,
        sample: impl Fn(&mut Gen, Arm) -> Sample,
    ) {
        for _ in 0..n {
            let window = ledger.windows()[0].clone();
            let arm = if rng.unit() < window.p() {
                Arm::Treatment
            } else {
                Arm::Control
            };
            let s = sample(rng, arm);
            ledger
                .execute(Command::Record(Box::new(testkit::observed(
                    &window, arm, s,
                ))))
                .unwrap();
        }
    }

    #[test]
    fn one_experiment_at_a_time_and_the_rest_of_the_refusals() {
        let mut l = ledger();
        l.execute(Command::Start(Box::new(spec()), Some(digest(1))))
            .unwrap();
        assert_eq!(
            l.decide(Command::Start(Box::new(spec()), None)),
            Err(LedgerError::ActiveExists)
        );
        let lever = spec().lever().key().clone();
        assert_eq!(
            l.decide(Command::Retire(lever.clone())),
            Err(LedgerError::InUse(lever.clone()))
        );
        assert_eq!(
            l.decide(Command::Written(lever.clone(), digest(2))),
            Err(LedgerError::NotApproved(lever))
        );
    }

    #[test]
    fn a_write_over_a_persons_edit_stops_the_experiment() {
        let mut l = ledger();
        l.execute(Command::Start(Box::new(spec()), Some(digest(1))))
            .unwrap();
        let lever = spec().lever().key().clone();
        l.execute(Command::PrepareWrite(lever.clone(), Some(digest(1))))
            .unwrap();
        l.execute(Command::Written(lever.clone(), digest(2)))
            .unwrap();
        let events = l
            .execute(Command::PrepareWrite(lever.clone(), Some(digest(9))))
            .unwrap();
        assert!(events.contains(&LedgerEvent::Decided(
            spec().id(),
            Verdict::Halted(TrustBreach::ExternalEdit)
        )));
        assert!(l.active().is_none());
        assert_eq!(l.blocked(), vec![&lever]);
        assert_eq!(
            l.decide(Command::Start(Box::new(spec()), Some(digest(9)))),
            Err(LedgerError::PutBackPending(lever.clone()))
        );
        l.execute(Command::ResolveExternalEdit(lever.clone()))
            .unwrap();
        assert!(
            l.decide(Command::Start(Box::new(spec()), Some(digest(9))))
                .is_ok()
        );
    }

    #[test]
    fn a_priced_saving_goes_through_confirmation_to_a_watch_and_replays() {
        let mut l = ledger();
        let mut log = l.execute(Command::Start(Box::new(spec()), None)).unwrap();
        let lever = spec().lever().key().clone();
        log.extend(
            l.execute(Command::PrepareWrite(lever.clone(), None))
                .unwrap(),
        );
        log.extend(
            l.execute(Command::Written(lever.clone(), digest(3)))
                .unwrap(),
        );
        let mut rng = Gen::new(21);
        let mut record = |l: &mut Ledger, log: &mut Vec<LedgerEvent>, n| {
            for _ in 0..n {
                let window = l.windows()[0].clone();
                let arm = if rng.unit() < window.p() {
                    Arm::Treatment
                } else {
                    Arm::Control
                };
                let write = if arm == Arm::Treatment { 400 } else { 1000 };
                let s = Sample::new(rng.around(write, 50), rng.around(500, 50), true);
                let run = testkit::observed(&window, arm, s);
                log.extend(l.execute(Command::Record(Box::new(run))).unwrap());
            }
        };
        record(&mut l, &mut log, 600);
        log.extend(l.execute(Command::Evaluate).unwrap());
        assert!(matches!(
            l.active().unwrap().window().kind(),
            WindowKind::Confirm(_)
        ));
        record(&mut l, &mut log, 1500);
        log.extend(l.execute(Command::Evaluate).unwrap());
        assert!(l.active().is_none());
        assert!(l.watches().contains_key(&lever));
        assert!(log.contains(&LedgerEvent::SurrogateHit(Mechanism::CacheExpiry)));
        let rebuilt = Ledger::replay(l.scope().clone(), l.policy, &log).unwrap();
        assert_eq!(rebuilt, l);
    }

    #[test]
    fn a_revert_gives_up_and_a_newer_harness_reopens() {
        let mut l = ledger();
        l.execute(Command::Start(Box::new(spec()), None)).unwrap();
        let lever = spec().lever().key().clone();
        l.execute(Command::PrepareWrite(lever.clone(), None))
            .unwrap();
        l.execute(Command::Written(lever.clone(), digest(4)))
            .unwrap();
        let mut rng = Gen::new(22);
        feed(&mut l, 100, &mut rng, |_, arm| {
            Sample::new(100, 100, arm == Arm::Control)
        });
        l.execute(Command::Evaluate).unwrap();
        assert!(l.active().is_none());
        assert_eq!(l.given_up().len(), 1);
        assert_eq!(l.pending_put_back().len(), 1);
        assert_eq!(
            l.decide(Command::CheckPutBack(lever.clone(), Some(digest(8))))
                .unwrap(),
            vec![LedgerEvent::ExternalEditFound(
                lever.clone(),
                spec().from().clone()
            )]
        );
        l.execute(Command::CheckPutBack(lever.clone(), Some(digest(4))))
            .unwrap();
        l.execute(Command::PutBackWritten(lever.clone())).unwrap();
        assert!(l.pending_put_back().is_empty());
        assert_eq!(
            l.decide(Command::Start(Box::new(spec()), None)),
            Err(LedgerError::GivenUp(lever))
        );
        let window = trial_window(
            WindowId::new(99),
            &Experiment::new(spec(), EvidenceClass::Priced),
        );
        let mut old = testkit::observed(&window, Arm::Control, Sample::new(1, 1, true));
        old.assigned = BTreeMap::new();
        let mut newer = old.clone();
        testkit::set_harness(&mut old, "2.0.0");
        let events = l.execute(Command::Record(Box::new(old))).unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, LedgerEvent::HarnessChanged(_)))
        );
        testkit::set_harness(&mut newer, "9.0.0");
        l.execute(Command::Record(Box::new(newer))).unwrap();
        assert!(l.given_up().is_empty());
        assert!(l.decide(Command::Start(Box::new(spec()), None)).is_ok());
    }

    #[test]
    fn a_halt_puts_back_without_giving_up() {
        let mut l = ledger();
        l.execute(Command::Start(Box::new(spec()), None)).unwrap();
        let lever = spec().lever().key().clone();
        l.execute(Command::PrepareWrite(lever.clone(), None))
            .unwrap();
        l.execute(Command::Written(lever.clone(), digest(5)))
            .unwrap();
        let window = l.windows()[0].clone();
        for i in 0..400 {
            let arm = if i % 10 == 0 {
                Arm::Control
            } else {
                Arm::Treatment
            };
            let mut run = testkit::observed(&window, arm, Sample::new(100, 100, true));
            run.assigned.insert(
                lever.clone(),
                Assigned {
                    window: window.id(),
                    position: if arm == Arm::Treatment {
                        window.treatment().clone()
                    } else {
                        window.control().clone()
                    },
                },
            );
            l.execute(Command::Record(Box::new(run))).unwrap();
        }
        let events = l.execute(Command::Evaluate).unwrap();
        assert!(events.contains(&LedgerEvent::Decided(
            spec().id(),
            Verdict::Halted(TrustBreach::ArmImbalance)
        )));
        assert!(l.given_up().is_empty());
        assert_eq!(l.pending_put_back().len(), 1);
    }

    #[test]
    fn a_stored_event_cannot_claim_evidence_the_ledger_would_not_give() {
        let mut l = ledger();
        let mut events = l.decide(Command::Start(Box::new(spec()), None)).unwrap();
        if let LedgerEvent::ExperimentStarted { evidence, .. } = &mut events[0] {
            *evidence = EvidenceClass::Behavioral;
        }
        assert!(matches!(
            l.apply(&events[0]),
            Err(LedgerError::UnfoldableEvent(_))
        ));
    }
}
