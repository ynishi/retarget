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
//! a newer harness ──> the windows of kept techniques reopen (as the Lab's watches do)
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
//! - A kept technique is put back only when it is shown not to fit; reaching
//!   the maximum of its window undecided leaves it kept.
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
use crate::domain::places::{PlaceEvent, Places, PlacesError};
use crate::domain::product::adoption::{Adoption, AdoptionState, UnfitReason};
use crate::domain::product::catalogue::{Catalogue, OnUndecided, Stage};
use crate::domain::product::finding::Finding;
use crate::domain::product::fit::{FitCall, Reading, fit};

/// A technique that did not fit, and when it may be tried again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfit {
    /// The technique.
    pub technique: TechniqueId,
    /// Why.
    pub reason: UnfitReason,
    /// The catalogue's version then; a newer one reopens it.
    pub catalogue: u32,
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
    /// A kept adoption's window was replaced, at the same stage.
    WindowReopened(LeverKey, Window),
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
    /// A step of writing or putting back a lever's place.
    Place(PlaceEvent),
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
    /// A step of writing was refused.
    #[error(transparent)]
    Places(#[from] PlacesError),
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

/// What a call does to an adoption.
enum Next {
    Advance,
    Keep,
    Unfit(UnfitReason),
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

    fn places_event(event: PlaceEvent) -> PortfolioEvent {
        PortfolioEvent::Place(event)
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
                let mut events = vec![PortfolioEvent::CatalogueLoaded(catalogue)];
                for u in &self.unfit {
                    if version > u.catalogue {
                        events.push(PortfolioEvent::RetryReopened(u.technique.clone()));
                    }
                }
                Ok(events)
            }
            PortfolioCommand::Found(findings, current, first_id) => {
                self.decide_found(findings, &current, first_id, &mut next_id)
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
                        // every technique that did not fit gets another chance
                        for u in &self.unfit {
                            events.push(PortfolioEvent::RetryReopened(u.technique.clone()));
                        }
                        // kept techniques are watched afresh on the new version
                        for (lever, adoption) in &self.adoptions {
                            if adoption.state() == AdoptionState::Kept
                                && let Some(window) = adoption.window_for(next_id(), Stage::Full)
                            {
                                events.push(PortfolioEvent::WindowReopened(lever.clone(), window));
                            }
                        }
                    }
                    Some(_) => {}
                }
                Ok(events)
            }
            PortfolioCommand::Evaluate => Ok(self.decide_evaluate(&mut next_id)),
            PortfolioCommand::PrepareWrite(lever, current) => {
                let adoption = self
                    .adoptions
                    .get(&lever)
                    .filter(|a| a.window().is_some())
                    .ok_or_else(|| PortfolioError::NotApplied(lever.clone()))?;
                let from = adoption.from().clone();
                Ok(
                    match self.places.prepare_write(&lever, current, from.clone()) {
                        Ok(approved) => vec![Self::places_event(approved)],
                        Err(_) => vec![
                            PortfolioEvent::AdoptionUnfit(
                                lever.clone(),
                                UnfitReason::Halted(TrustBreach::ExternalEdit),
                            ),
                            Self::places_event(PlaceEvent::ExternalEditFound(lever, from)),
                        ],
                    },
                )
            }
            PortfolioCommand::Written(lever, digest) => {
                let owned = self
                    .adoptions
                    .get(&lever)
                    .is_some_and(|a| a.window().is_some());
                Ok(self
                    .places
                    .written(&lever, digest, owned)?
                    .into_iter()
                    .map(Self::places_event)
                    .collect())
            }
            PortfolioCommand::CheckPutBack(lever, current) => Ok(vec![Self::places_event(
                self.places.check_put_back(&lever, current)?,
            )]),
            PortfolioCommand::PutBackWritten(lever) => Ok(vec![Self::places_event(
                self.places.put_back_written(&lever)?,
            )]),
            PortfolioCommand::ResolveExternalEdit(lever) => {
                Ok(vec![Self::places_event(self.places.resolve(&lever)?)])
            }
        }
    }

    fn decide_found(
        &self,
        findings: Vec<Finding>,
        current: &BTreeMap<LeverKey, PositionId>,
        first_id: u128,
        next_id: &mut impl FnMut() -> WindowId,
    ) -> Result<Vec<PortfolioEvent>, PortfolioError> {
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
            let Some(from) = current.get(&lever).cloned() else {
                continue;
            };
            let id = AdoptionId::new(next_adoption);
            next_adoption += 1;
            events.push(PortfolioEvent::AdoptionShown(
                id,
                technique.id().clone(),
                from.clone(),
            ));
            let applicable = technique.lever().surface().applicability() == Applicability::Apply;
            if technique.automatic() && applicable {
                let adoption = Adoption::shown(id, technique.clone(), from);
                let stage = adoption.first_stage();
                if let Some(window) = adoption.window_for(next_id(), stage) {
                    events.push(PortfolioEvent::StageAdvanced(lever.clone(), stage, window));
                }
            }
            used.extend(technique.mechanisms().iter().copied());
            levers.insert(lever);
        }
        Ok(events)
    }

    fn decide_evaluate(&self, next_id: &mut impl FnMut() -> WindowId) -> Vec<PortfolioEvent> {
        let mut events = Vec::new();
        for (lever, adoption) in &self.adoptions {
            let state = adoption.state();
            if state == AdoptionState::Shown {
                continue;
            }
            let (call, _) = fit(adoption, &self.runs);
            let put_back_undecided =
                adoption.technique().fit().on_undecided == OnUndecided::PutBack;
            let partial = matches!(state, AdoptionState::Applying(Stage::Partial(_)));
            let next = match (call, state) {
                (FitCall::Pending, _) => continue,
                (FitCall::Unfit(reason), _) => Next::Unfit(reason),
                // a kept technique stays kept until it is shown not to fit
                (FitCall::Fits | FitCall::Undecided, AdoptionState::Kept) => continue,
                (FitCall::Undecided, _) if put_back_undecided => {
                    Next::Unfit(UnfitReason::Undecided)
                }
                (FitCall::Fits | FitCall::Undecided, _) if partial => Next::Advance,
                (FitCall::Fits | FitCall::Undecided, _) => Next::Keep,
            };
            events.push(PortfolioEvent::FitReached(lever.clone(), call));
            match next {
                Next::Advance => {
                    if let Some(window) = adoption.window_for(next_id(), Stage::Full) {
                        events.push(PortfolioEvent::StageAdvanced(
                            lever.clone(),
                            Stage::Full,
                            window,
                        ));
                    }
                }
                Next::Keep => events.push(PortfolioEvent::AdoptionKept(lever.clone())),
                Next::Unfit(reason) => {
                    events.push(PortfolioEvent::AdoptionUnfit(lever.clone(), reason));
                    if let Some(due) = self.places.closed(lever, adoption.from()) {
                        events.push(Self::places_event(due));
                    }
                }
            }
        }
        events
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
                self.adoptions
                    .get_mut(lever)
                    .ok_or(PortfolioError::UnfoldableEvent("no adoption to advance"))?
                    .advance(*stage, window.clone());
            }
            PortfolioEvent::WindowReopened(lever, window) => {
                self.bump(window);
                self.adoptions
                    .get_mut(lever)
                    .ok_or(PortfolioError::UnfoldableEvent("no adoption to reopen"))?
                    .reopen(window.clone());
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
                });
            }
            PortfolioEvent::RetryReopened(technique) => {
                self.unfit.retain(|u| &u.technique != technique);
            }
            PortfolioEvent::RunRecorded(run) => self.runs.push(run.clone()),
            PortfolioEvent::HarnessChanged(version) => self.harness = Some(*version),
            PortfolioEvent::Place(event) => self.places.apply(event)?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::assign::Arm;
    use crate::domain::product::catalogue::Technique;
    use crate::domain::product::finding::OFF_MAIN_CACHE_REWRITES;
    use crate::domain::testkit::{self, Gen, Sample};

    fn catalogue_with(on_undecided: OnUndecided) -> Catalogue {
        let mut parts = testkit::technique_parts();
        parts.fit.on_undecided = on_undecided;
        Catalogue::new(1, vec![Technique::new(parts).unwrap()]).unwrap()
    }

    fn portfolio_with(catalogue: Catalogue) -> Portfolio {
        let mut p = Portfolio::new("repo".parse().unwrap());
        p.execute(PortfolioCommand::LoadCatalogue(Box::new(catalogue)))
            .unwrap();
        p
    }

    fn portfolio() -> Portfolio {
        portfolio_with(catalogue_with(OnUndecided::KeepWatching))
    }

    fn lever() -> LeverKey {
        "cache.off_main_ttl".parse().unwrap()
    }

    fn found(p: &mut Portfolio) -> Vec<PortfolioEvent> {
        let finding = Finding {
            pattern: OFF_MAIN_CACHE_REWRITES.parse().unwrap(),
            share: 0.3,
            runs: 20,
        };
        let current = BTreeMap::from([(lever(), "5m".parse().unwrap())]);
        p.execute(PortfolioCommand::Found(vec![finding], current, 100))
            .unwrap()
    }

    fn allow_and_write(p: &mut Portfolio, log: &mut Vec<PortfolioEvent>) {
        log.extend(
            p.execute(PortfolioCommand::Allow("subagent-ttl-1h".parse().unwrap()))
                .unwrap(),
        );
        log.extend(
            p.execute(PortfolioCommand::PrepareWrite(lever(), None))
                .unwrap(),
        );
        log.extend(
            p.execute(PortfolioCommand::Written(lever(), Digest::new([1; 32])))
                .unwrap(),
        );
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
    fn a_contraindicated_technique_is_not_shown() {
        let mut parts = testkit::technique_parts();
        parts.contraindications = vec!["rework".parse().unwrap()];
        let mut p =
            portfolio_with(Catalogue::new(1, vec![Technique::new(parts).unwrap()]).unwrap());
        let findings = vec![
            Finding {
                pattern: OFF_MAIN_CACHE_REWRITES.parse().unwrap(),
                share: 0.3,
                runs: 20,
            },
            Finding {
                pattern: "rework".parse().unwrap(),
                share: 0.2,
                runs: 20,
            },
        ];
        let current = BTreeMap::from([(lever(), "5m".parse().unwrap())]);
        let events = p
            .execute(PortfolioCommand::Found(findings, current, 1))
            .unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PortfolioEvent::AdoptionShown(..)))
        );
    }

    #[test]
    fn a_fitting_technique_goes_from_shown_to_kept_and_replays() {
        let mut p = portfolio();
        let catalogue = p.catalogue.clone().unwrap();
        let mut log = Vec::new();
        log.extend(found(&mut p));
        allow_and_write(&mut p, &mut log);
        let mut rng = Gen::new(31);
        feed(&mut p, 1200, &mut rng, &mut log, (1000, 400));
        assert_eq!(p.adoptions()[&lever()].state(), AdoptionState::Kept);
        assert!(p.readings()[0].observed.unwrap().get() < 0.0);
        // the catalogue was loaded before the log started
        let mut all = vec![PortfolioEvent::CatalogueLoaded(Box::new(catalogue))];
        all.extend(log);
        assert_eq!(Portfolio::replay(p.scope().clone(), &all).unwrap(), p);
    }

    #[test]
    fn a_kept_technique_is_not_put_back_for_reaching_its_maximum() {
        let mut parts = testkit::technique_parts();
        parts.fit.on_undecided = OnUndecided::PutBack;
        let mut stopping = testkit::stopping_parts();
        stopping.max_runs = 200;
        stopping.max_confirm_runs = 200;
        parts.fit.stopping = crate::domain::measure::StoppingRule::new(stopping).unwrap();
        let mut p =
            portfolio_with(Catalogue::new(1, vec![Technique::new(parts).unwrap()]).unwrap());
        let mut log = Vec::new();
        found(&mut p);
        allow_and_write(&mut p, &mut log);
        let mut rng = Gen::new(33);
        // clearly cheaper at first, then indistinguishable past the maximum
        feed(&mut p, 400, &mut rng, &mut log, (1000, 400));
        assert_eq!(p.adoptions()[&lever()].state(), AdoptionState::Kept);
        feed(&mut p, 400, &mut rng, &mut log, (1000, 1000));
        assert_eq!(p.adoptions()[&lever()].state(), AdoptionState::Kept);
    }

    #[test]
    fn a_technique_that_makes_things_worse_is_put_back_and_not_tried_again() {
        let mut p = portfolio();
        found(&mut p);
        let mut log = Vec::new();
        allow_and_write(&mut p, &mut log);
        let mut rng = Gen::new(32);
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

    #[test]
    fn a_newer_harness_reopens_the_window_of_a_kept_technique() {
        let mut p = portfolio();
        let mut log = Vec::new();
        found(&mut p);
        allow_and_write(&mut p, &mut log);
        let mut rng = Gen::new(34);
        feed(&mut p, 1200, &mut rng, &mut log, (1000, 400));
        let before = p.windows()[0].id();
        let window = p.windows()[0].clone();
        let mut run = testkit::observed(&window, Arm::Treatment, Sample::new(400, 500, true));
        testkit::set_harness(&mut run, "9.0.0");
        let events = p.execute(PortfolioCommand::Record(Box::new(run))).unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PortfolioEvent::WindowReopened(..)))
        );
        assert_ne!(p.windows()[0].id(), before);
        assert_eq!(p.adoptions()[&lever()].state(), AdoptionState::Kept);
    }
}
