//! The portfolio of a scope: the techniques it has found, applied and kept,
//! the ones that did not fit, and what it still has to put back.
//!
//! As in the Lab's ledger, every decision is taken in [`Portfolio::decide`],
//! derived events included, and [`Portfolio::apply`] only folds events, so a
//! portfolio rebuilt from its events is the same portfolio. Writing and
//! putting back go through [`crate::domain::places`].
//!
//! ```text
//! LoadCatalogue ──> Found(findings, current positions) ──> AdoptionShown (+ StageAdvanced if automatic)
//! Allow(technique) ──> StageAdvanced (first stage)
//! Record / Evaluate ──> fit ──> StageAdvanced (Full) | AdoptionKept | AdoptionUnfit + PutBackDue
//! ```
//!
//! # Invariants
//!
//! - One adoption per lever; adoptions whose mechanisms overlap are not run
//!   together, those that do not overlap are (their assignments are
//!   independent, so they do not bias each other's comparison).
//! - A technique is not adopted while a pattern it is contraindicated for is
//!   found, nor written in the user's own scope (it stays shown).
//! - A technique that did not fit is not adopted again until the harness or
//!   the catalogue is newer than when it failed.
//! - What a technique is expected to do comes from the catalogue only.
//!
//! # Errors
//!
//! [`PortfolioError`] says which rule a command broke; the portfolio is
//! unchanged. Load a catalogue first, allow only what is shown, and write only
//! what was approved.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::assign::{ObservedRun, Window};
use crate::domain::lever::Applicability;
use crate::domain::measure::TrustBreach;
use crate::domain::mechanism::Mechanism;
use crate::domain::name::{
    AdoptionId, Digest, HarnessVersion, LeverKey, PositionId, Scope, TechniqueId, WindowId,
};
use crate::domain::places::Places;
use crate::domain::product::adoption::{Adoption, AdoptionState, UnfitReason};
use crate::domain::product::catalogue::{Catalogue, OnUndecided, Stage};
use crate::domain::product::finding::Finding;
use crate::domain::product::fit::{FitCall, Reading, fit};
use crate::domain::put_back::PutBack;

/// A technique that did not fit, and when it may be tried again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfit {
    /// The technique.
    pub technique: TechniqueId,
    /// Why.
    pub reason: UnfitReason,
    /// The catalogue's version then; a newer one reopens it.
    pub catalogue: u32,
    /// The harness's version then; a newer one reopens it.
    pub harness: Option<HarnessVersion>,
}

/// Something the portfolio decided or recorded.
#[derive(Debug, Clone, PartialEq)]
pub enum PortfolioEvent {
    /// A catalogue was loaded.
    CatalogueLoaded(Box<Catalogue>),
    /// Patterns were found.
    Found(Vec<Finding>),
    /// A technique is shown for a lever, from the position the lever had.
    AdoptionShown(AdoptionId, TechniqueId, PositionId),
    /// An adoption moved to a stage, with the window it is checked on.
    StageAdvanced(LeverKey, Stage, Window),
    /// A window reached a call.
    FitReached(LeverKey, FitCall),
    /// An adoption fits and is kept.
    AdoptionKept(LeverKey),
    /// An adoption did not fit and ends.
    AdoptionUnfit(LeverKey, UnfitReason),
    /// A technique that did not fit may be tried again.
    RetryReopened(TechniqueId),
    /// A run was recorded.
    RunRecorded(ObservedRun),
    /// A newer harness version was seen.
    HarnessChanged(HarnessVersion),
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
}

/// What the portfolio is asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum PortfolioCommand {
    /// Use this catalogue from now on.
    LoadCatalogue(Box<Catalogue>),
    /// Patterns found in recent runs, the levers' current positions, and the
    /// first of the ids to give new adoptions.
    Found(Vec<Finding>, BTreeMap<LeverKey, PositionId>, u128),
    /// The customer allows a shown technique.
    Allow(TechniqueId),
    /// Record a finished run.
    Record(Box<ObservedRun>),
    /// Check every applied technique.
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
}

/// Why a command was refused, or an event could not be folded.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PortfolioError {
    /// No catalogue was loaded.
    #[error("no catalogue was loaded")]
    NoCatalogue,
    /// No shown adoption has this technique.
    #[error("technique `{0}` is not shown")]
    NotShown(TechniqueId),
    /// The technique's lever is written only in the user's own scope.
    #[error("technique `{0}` is only shown: its lever is in the user's own scope")]
    ShowOnly(TechniqueId),
    /// No applied adoption writes this lever.
    #[error("nothing writes lever `{0}`")]
    NotApplied(LeverKey),
    /// The write was not approved first.
    #[error("the write of lever `{0}` was not approved")]
    NotApproved(LeverKey),
    /// The lever has nothing to put back.
    #[error("lever `{0}` has nothing to put back")]
    NothingToPutBack(LeverKey),
    /// The lever is not blocked by an edit.
    #[error("lever `{0}` is not blocked")]
    NotBlocked(LeverKey),
    /// An event does not fit the state it is folded into.
    #[error("an event does not fit the portfolio: {0}")]
    UnfoldableEvent(&'static str),
}

/// The portfolio of one scope.
#[derive(Debug, Clone, PartialEq)]
pub struct Portfolio {
    scope: Scope,
    catalogue: Option<Catalogue>,
    harness: Option<HarnessVersion>,
    next_window: u64,
    adoptions: BTreeMap<LeverKey, Adoption>,
    unfit: Vec<Unfit>,
    findings: Vec<Finding>,
    places: Places,
    runs: Vec<ObservedRun>,
}

impl Portfolio {
    /// An empty portfolio.
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            catalogue: None,
            harness: None,
            next_window: 1,
            adoptions: BTreeMap::new(),
            unfit: Vec::new(),
            findings: Vec::new(),
            places: Places::default(),
            runs: Vec::new(),
        }
    }

    /// A portfolio rebuilt from its events.
    pub fn replay(scope: Scope, events: &[PortfolioEvent]) -> Result<Self, PortfolioError> {
        let mut portfolio = Self::new(scope);
        for event in events {
            portfolio.apply(event)?;
        }
        Ok(portfolio)
    }

    /// Decide a command and fold its events; the events are returned for the store.
    pub fn execute(
        &mut self,
        command: PortfolioCommand,
    ) -> Result<Vec<PortfolioEvent>, PortfolioError> {
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
    /// The adoptions, by lever.
    pub fn adoptions(&self) -> &BTreeMap<LeverKey, Adoption> {
        &self.adoptions
    }
    /// The techniques that did not fit.
    pub fn unfit(&self) -> &[Unfit] {
        &self.unfit
    }
    /// The patterns found last.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }
    /// The windows runs are assigned in now.
    pub fn windows(&self) -> Vec<&Window> {
        self.adoptions
            .values()
            .filter_map(Adoption::window)
            .collect()
    }
    /// The places of the levers, and what must be put back.
    pub fn places(&self) -> &Places {
        &self.places
    }
    /// What every adoption expects and shows so far.
    pub fn readings(&self) -> Vec<Reading> {
        self.adoptions
            .values()
            .map(|a| fit(a, &self.runs).1)
            .collect()
    }

    fn catalogue(&self) -> Result<&Catalogue, PortfolioError> {
        self.catalogue.as_ref().ok_or(PortfolioError::NoCatalogue)
    }

    fn mechanisms_in_use(&self) -> BTreeSet<Mechanism> {
        self.adoptions
            .values()
            .flat_map(|a| a.technique().mechanisms().iter().copied())
            .collect()
    }

    fn is_unfit(&self, technique: &TechniqueId) -> bool {
        self.unfit.iter().any(|u| &u.technique == technique)
    }

    /// Decide a command: the events that record it, or why it is refused.
    pub fn decide(&self, command: PortfolioCommand) -> Result<Vec<PortfolioEvent>, PortfolioError> {
        let mut ids = self.next_window;
        let mut next_id = move || {
            let id = WindowId::new(ids);
            ids += 1;
            id
        };
        match command {
            PortfolioCommand::LoadCatalogue(catalogue) => {
                let version = catalogue.version();
                let mut events = Vec::new();
                for u in &self.unfit {
                    if version > u.catalogue {
                        events.push(PortfolioEvent::RetryReopened(u.technique.clone()));
                    }
                }
                events.insert(0, PortfolioEvent::CatalogueLoaded(catalogue));
                Ok(events)
            }
            PortfolioCommand::Found(findings, current, first_id) => {
                let catalogue = self.catalogue()?;
                let found: BTreeSet<_> = findings.iter().map(|f| f.pattern.clone()).collect();
                let mut used = self.mechanisms_in_use();
                let mut levers: BTreeSet<LeverKey> = self.adoptions.keys().cloned().collect();
                let mut events = vec![PortfolioEvent::Found(findings.clone())];
                let mut next_adoption = first_id;
                for finding in &findings {
                    let chosen = catalogue.for_pattern(&finding.pattern).find(|t| {
                        let lever = t.lever().key();
                        !t.contraindications().iter().any(|c| found.contains(c))
                            && !levers.contains(lever)
                            && t.mechanisms().is_disjoint(&used)
                            && !self.is_unfit(t.id())
                            && !self.places.is_pending(lever)
                            && current.get(lever).is_some_and(|at| at != t.to())
                    });
                    let Some(technique) = chosen else { continue };
                    let lever = technique.lever().key().clone();
                    let from =
                        current
                            .get(&lever)
                            .cloned()
                            .ok_or(PortfolioError::UnfoldableEvent(
                                "a chosen lever has no position",
                            ))?;
                    let id = AdoptionId::new(next_adoption);
                    next_adoption += 1;
                    events.push(PortfolioEvent::AdoptionShown(
                        id,
                        technique.id().clone(),
                        from.clone(),
                    ));
                    let applicable =
                        technique.lever().surface().applicability() == Applicability::Apply;
                    if technique.automatic() && applicable {
                        let adoption = Adoption::shown(id, technique.clone(), from);
                        let stage = adoption.first_stage();
                        if let Some(window) = adoption.window_for(next_id(), stage) {
                            events.push(PortfolioEvent::StageAdvanced(
                                lever.clone(),
                                stage,
                                window,
                            ));
                        }
                    }
                    used.extend(technique.mechanisms().iter().copied());
                    levers.insert(lever);
                }
                Ok(events)
            }
            PortfolioCommand::Allow(technique) => {
                let adoption = self
                    .adoptions
                    .values()
                    .find(|a| a.technique().id() == &technique && a.state() == AdoptionState::Shown)
                    .ok_or_else(|| PortfolioError::NotShown(technique.clone()))?;
                if adoption.technique().lever().surface().applicability() != Applicability::Apply {
                    return Err(PortfolioError::ShowOnly(technique));
                }
                let stage = adoption.first_stage();
                let window = adoption.window_for(next_id(), stage).ok_or(
                    PortfolioError::UnfoldableEvent("a first stage without a window"),
                )?;
                Ok(vec![PortfolioEvent::StageAdvanced(
                    adoption.technique().lever().key().clone(),
                    stage,
                    window,
                )])
            }
            PortfolioCommand::Record(run) => {
                let version = run.run.start().harness;
                let mut events = vec![PortfolioEvent::RunRecorded(*run)];
                match self.harness {
                    None => events.push(PortfolioEvent::HarnessChanged(version)),
                    Some(known) if version > known => {
                        events.push(PortfolioEvent::HarnessChanged(version));
                        for u in &self.unfit {
                            if u.harness.is_none_or(|h| version > h) {
                                events.push(PortfolioEvent::RetryReopened(u.technique.clone()));
                            }
                        }
                    }
                    Some(_) => {}
                }
                Ok(events)
            }
            PortfolioCommand::Evaluate => {
                let mut events = Vec::new();
                for (lever, adoption) in &self.adoptions {
                    let state = adoption.state();
                    if state == AdoptionState::Shown {
                        continue;
                    }
                    let (call, _) = fit(adoption, &self.runs);
                    let on_undecided = adoption.technique().fit().on_undecided;
                    let next = match (call, state) {
                        (FitCall::Pending, _) => continue,
                        (FitCall::Fits, AdoptionState::Applying(Stage::Partial(_))) => {
                            Step::Advance
                        }
                        (FitCall::Fits, AdoptionState::Applying(_)) => Step::Keep,
                        (FitCall::Fits, _) => continue,
                        (FitCall::Unfit(reason), _) => Step::Unfit(reason),
                        (FitCall::Undecided, _) if on_undecided == OnUndecided::PutBack => {
                            Step::Unfit(UnfitReason::Undecided)
                        }
                        (FitCall::Undecided, AdoptionState::Applying(Stage::Partial(_))) => {
                            Step::Advance
                        }
                        (FitCall::Undecided, AdoptionState::Applying(_)) => Step::Keep,
                        (FitCall::Undecided, _) => continue,
                    };
                    events.push(PortfolioEvent::FitReached(lever.clone(), call));
                    match next {
                        Step::Advance => {
                            if let Some(window) = adoption.window_for(next_id(), Stage::Full) {
                                events.push(PortfolioEvent::StageAdvanced(
                                    lever.clone(),
                                    Stage::Full,
                                    window,
                                ));
                            }
                        }
                        Step::Keep => events.push(PortfolioEvent::AdoptionKept(lever.clone())),
                        Step::Unfit(reason) => {
                            events.push(PortfolioEvent::AdoptionUnfit(lever.clone(), reason));
                            if self.places.wrote(lever) {
                                events.push(PortfolioEvent::PutBackDue(
                                    lever.clone(),
                                    adoption.from().clone(),
                                ));
                            }
                        }
                    }
                }
                Ok(events)
            }
            PortfolioCommand::PrepareWrite(lever, current) => {
                let adoption = self
                    .adoptions
                    .get(&lever)
                    .filter(|a| a.window().is_some())
                    .ok_or_else(|| PortfolioError::NotApplied(lever.clone()))?;
                Ok(match self.places.check(&lever, current.as_ref()) {
                    PutBack::Proceed => vec![PortfolioEvent::WriteApproved(lever, current)],
                    PutBack::ExternalEdit => vec![
                        PortfolioEvent::AdoptionUnfit(
                            lever.clone(),
                            UnfitReason::Halted(TrustBreach::ExternalEdit),
                        ),
                        PortfolioEvent::ExternalEditFound(lever, adoption.from().clone()),
                    ],
                })
            }
            PortfolioCommand::Written(lever, digest) => {
                if self.places.write_approved(&lever) {
                    Ok(vec![PortfolioEvent::Written(lever, digest)])
                } else {
                    Err(PortfolioError::NotApproved(lever))
                }
            }
            PortfolioCommand::CheckPutBack(lever, current) => {
                let to = self
                    .places
                    .due(&lever)
                    .cloned()
                    .ok_or_else(|| PortfolioError::NothingToPutBack(lever.clone()))?;
                Ok(match self.places.check(&lever, current.as_ref()) {
                    PutBack::Proceed => vec![PortfolioEvent::PutBackApproved(lever)],
                    PutBack::ExternalEdit => vec![PortfolioEvent::ExternalEditFound(lever, to)],
                })
            }
            PortfolioCommand::PutBackWritten(lever) => {
                if self.places.put_back_approved(&lever) {
                    Ok(vec![PortfolioEvent::PutBackDone(lever)])
                } else {
                    Err(PortfolioError::NotApproved(lever))
                }
            }
            PortfolioCommand::ResolveExternalEdit(lever) => {
                if self.places.is_blocked(&lever) {
                    Ok(vec![PortfolioEvent::ExternalEditResolved(lever)])
                } else {
                    Err(PortfolioError::NotBlocked(lever))
                }
            }
        }
    }

    fn bump(&mut self, window: &Window) {
        self.next_window = self.next_window.max(window.id().get() + 1);
    }

    /// Fold an event into the state. Nothing is decided here; an event that
    /// does not fit is refused.
    pub fn apply(&mut self, event: &PortfolioEvent) -> Result<(), PortfolioError> {
        match event {
            PortfolioEvent::CatalogueLoaded(catalogue) => {
                self.catalogue = Some(catalogue.as_ref().clone());
            }
            PortfolioEvent::Found(findings) => self.findings = findings.clone(),
            PortfolioEvent::AdoptionShown(id, technique, from) => {
                let technique = self.catalogue()?.get(technique).cloned().ok_or(
                    PortfolioError::UnfoldableEvent("a technique not in the catalogue"),
                )?;
                let lever = technique.lever().key().clone();
                self.adoptions
                    .insert(lever, Adoption::shown(*id, technique, from.clone()));
            }
            PortfolioEvent::StageAdvanced(lever, stage, window) => {
                self.bump(window);
                let adoption = self
                    .adoptions
                    .get_mut(lever)
                    .ok_or(PortfolioError::UnfoldableEvent("no adoption to advance"))?;
                adoption.advance(*stage, window.clone());
            }
            PortfolioEvent::FitReached(..) => {}
            PortfolioEvent::AdoptionKept(lever) => {
                self.adoptions
                    .get_mut(lever)
                    .ok_or(PortfolioError::UnfoldableEvent("no adoption to keep"))?
                    .keep();
            }
            PortfolioEvent::AdoptionUnfit(lever, reason) => {
                let adoption = self
                    .adoptions
                    .remove(lever)
                    .ok_or(PortfolioError::UnfoldableEvent("no adoption to end"))?;
                let catalogue = self.catalogue()?.version();
                self.unfit.push(Unfit {
                    technique: adoption.technique().id().clone(),
                    reason: *reason,
                    catalogue,
                    harness: self.harness,
                });
            }
            PortfolioEvent::RetryReopened(technique) => {
                self.unfit.retain(|u| &u.technique != technique);
            }
            PortfolioEvent::RunRecorded(run) => self.runs.push(run.clone()),
            PortfolioEvent::HarnessChanged(version) => self.harness = Some(*version),
            PortfolioEvent::WriteApproved(lever, current) => {
                self.places.approve_write(lever, *current);
            }
            PortfolioEvent::Written(lever, digest) => {
                if !self.places.written(lever, *digest) {
                    return Err(PortfolioError::UnfoldableEvent(
                        "a write that was not approved",
                    ));
                }
            }
            PortfolioEvent::ExternalEditFound(lever, to) => {
                self.places.external_edit(lever, to.clone());
            }
            PortfolioEvent::PutBackDue(lever, to) => self.places.put_back_due(lever, to.clone()),
            PortfolioEvent::PutBackApproved(lever) => {
                if !self.places.approve_put_back(lever) {
                    return Err(PortfolioError::UnfoldableEvent("nothing due to put back"));
                }
            }
            PortfolioEvent::PutBackDone(lever) | PortfolioEvent::ExternalEditResolved(lever) => {
                self.places.forget(lever);
            }
        }
        Ok(())
    }
}

/// What a call does to an adoption.
enum Step {
    Advance,
    Keep,
    Unfit(UnfitReason),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::assign::Arm;
    use crate::domain::product::catalogue::Technique;
    use crate::domain::product::finding::OFF_MAIN_CACHE_REWRITES;
    use crate::domain::testkit::{self, Gen, Sample};

    fn portfolio() -> Portfolio {
        let mut p = Portfolio::new("repo".parse().unwrap());
        let catalogue =
            Catalogue::new(1, vec![Technique::new(testkit::technique_parts()).unwrap()]).unwrap();
        p.execute(PortfolioCommand::LoadCatalogue(Box::new(catalogue)))
            .unwrap();
        p
    }

    fn found(p: &mut Portfolio) -> Vec<PortfolioEvent> {
        let finding = Finding {
            pattern: OFF_MAIN_CACHE_REWRITES.parse().unwrap(),
            share: 0.3,
            runs: 20,
        };
        let lever: LeverKey = "cache.off_main_ttl".parse().unwrap();
        let current = BTreeMap::from([(lever, "5m".parse().unwrap())]);
        p.execute(PortfolioCommand::Found(vec![finding], current, 100))
            .unwrap()
    }

    fn feed(
        p: &mut Portfolio,
        n: usize,
        rng: &mut Gen,
        log: &mut Vec<PortfolioEvent>,
        write: (u64, u64),
    ) {
        for _ in 0..n {
            let Some(window) = p.windows().first().map(|w| (*w).clone()) else {
                return;
            };
            let arm = if rng.unit() < window.p() {
                Arm::Treatment
            } else {
                Arm::Control
            };
            let w = if arm == Arm::Treatment {
                write.1
            } else {
                write.0
            };
            let s = Sample::new(rng.around(w, 50), rng.around(500, 50), true);
            let run = testkit::observed(&window, arm, s);
            log.extend(p.execute(PortfolioCommand::Record(Box::new(run))).unwrap());
            log.extend(p.execute(PortfolioCommand::Evaluate).unwrap());
        }
    }

    #[test]
    fn a_found_pattern_is_shown_with_what_it_is_expected_to_do() {
        let mut p = portfolio();
        found(&mut p);
        let readings = p.readings();
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].state, AdoptionState::Shown);
        assert!(p.windows().is_empty());
        assert!(matches!(
            p.decide(PortfolioCommand::Allow("nope".parse().unwrap())),
            Err(PortfolioError::NotShown(_))
        ));
    }

    #[test]
    fn a_fitting_technique_goes_from_shown_to_kept_and_replays() {
        let mut p = portfolio();
        let mut log = Vec::new();
        log.extend(found(&mut p));
        let technique: TechniqueId = "subagent-ttl-1h".parse().unwrap();
        log.extend(p.execute(PortfolioCommand::Allow(technique)).unwrap());
        let lever: LeverKey = "cache.off_main_ttl".parse().unwrap();
        log.extend(
            p.execute(PortfolioCommand::PrepareWrite(lever.clone(), None))
                .unwrap(),
        );
        log.extend(
            p.execute(PortfolioCommand::Written(
                lever.clone(),
                Digest::new([1; 32]),
            ))
            .unwrap(),
        );
        let mut rng = Gen::new(31);
        feed(&mut p, 1200, &mut rng, &mut log, (1000, 400));
        assert_eq!(p.adoptions()[&lever].state(), AdoptionState::Kept);
        let reading = &p.readings()[0];
        assert!(reading.observed.unwrap().get() < 0.0);
        // the catalogue was loaded before the log started
        let rebuilt = Portfolio::replay(
            p.scope().clone(),
            &[
                vec![PortfolioEvent::CatalogueLoaded(Box::new(
                    p.catalogue.clone().unwrap(),
                ))],
                log,
            ]
            .concat(),
        )
        .unwrap();
        assert_eq!(rebuilt, p);
    }

    #[test]
    fn a_technique_that_makes_things_worse_is_put_back_and_not_tried_again() {
        let mut p = portfolio();
        found(&mut p);
        let lever: LeverKey = "cache.off_main_ttl".parse().unwrap();
        p.execute(PortfolioCommand::Allow("subagent-ttl-1h".parse().unwrap()))
            .unwrap();
        p.execute(PortfolioCommand::PrepareWrite(lever.clone(), None))
            .unwrap();
        p.execute(PortfolioCommand::Written(
            lever.clone(),
            Digest::new([2; 32]),
        ))
        .unwrap();
        let mut rng = Gen::new(32);
        let mut log = Vec::new();
        feed(&mut p, 1200, &mut rng, &mut log, (500, 1500));
        assert!(p.adoptions().is_empty());
        assert_eq!(p.unfit().len(), 1);
        assert_eq!(p.places().pending_put_back().len(), 1);
        let events = found(&mut p);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PortfolioEvent::AdoptionShown(..)))
        );
    }
}
