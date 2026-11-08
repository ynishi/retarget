//! An adoption: one technique applied, or shown, in one scope.
//!
//! ```text
//! Shown ──(allowed, or automatic)──> Applying(Partial) ──fits──> Applying(Full) ──fits──> Kept
//!                                          │                           │                   │
//!                                          └──── not fit ──────────────┴───── not fit ─────┴──> Unfit
//! ```
//!
//! # Invariants
//!
//! - The stage only moves forward: shown, then a share of runs, then all but
//!   a holdout.
//! - An adoption applies its technique's step from the position the lever had
//!   when it was found to the technique's target.

use crate::domain::assign::{Window, WindowKind, WindowParts};
use crate::domain::measure::{Guardrail, TrustBreach};
use crate::domain::name::{AdoptionId, PositionId, Ratio, WindowId};
use crate::domain::product::catalogue::{Rollout, Stage, Technique};

/// Why a technique did not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnfitReason {
    /// The cost per successful run was shown worse than the margin allows.
    Worse,
    /// A guardrail was shown worse than its margin.
    GuardrailBreached(Guardrail),
    /// Too many failures in a row.
    Tripped,
    /// A trust check broke, or a person edited the lever's place.
    Halted(TrustBreach),
    /// No call by the maximum, and the catalogue says to put it back.
    Undecided,
}

/// Where an adoption is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AdoptionState {
    /// Shown, waiting to be allowed.
    Shown,
    /// Applied at a stage, being checked.
    Applying(Stage),
    /// Fits; still watched against the holdout.
    Kept,
}

/// One technique in one scope.
#[derive(Debug, Clone, PartialEq)]
pub struct Adoption {
    id: AdoptionId,
    technique: Technique,
    from: PositionId,
    state: AdoptionState,
    window: Option<Window>,
}

impl Adoption {
    pub(crate) fn shown(id: AdoptionId, technique: Technique, from: PositionId) -> Self {
        Self {
            id,
            technique,
            from,
            state: AdoptionState::Shown,
            window: None,
        }
    }
    /// Its id.
    pub fn id(&self) -> AdoptionId {
        self.id
    }
    /// The technique.
    pub fn technique(&self) -> &Technique {
        &self.technique
    }
    /// Where the lever was when the pattern was found, and where it goes back to.
    pub fn from(&self) -> &PositionId {
        &self.from
    }
    /// Where it is.
    pub fn state(&self) -> AdoptionState {
        self.state
    }
    /// The window of runs it is checked on, once applied.
    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }
    /// The stage it moves to when allowed.
    pub(crate) fn first_stage(&self) -> Stage {
        match self.technique.rollout() {
            Rollout::Partial(share) => Stage::Partial(share),
            Rollout::Full => Stage::Full,
        }
    }
    /// The window of a stage: the share of runs on the technique's position.
    pub(crate) fn window_for(&self, id: WindowId, stage: Stage) -> Option<Window> {
        let p = match stage {
            Stage::Shown => return None,
            Stage::Partial(share) => share,
            Stage::Full => Ratio::new(1.0 - self.technique.fit().holdout.get()).ok()?,
        };
        let lever = self.technique.lever();
        Some(Window::new(WindowParts {
            id,
            kind: WindowKind::Adoption(self.id),
            lever: lever.key().clone(),
            control: self.from.clone(),
            treatment: self.technique.to().clone(),
            p,
            task_class: None,
            scope: lever.scope().clone(),
        }))
    }
    pub(crate) fn advance(&mut self, stage: Stage, window: Window) {
        self.state = AdoptionState::Applying(stage);
        self.window = Some(window);
    }
    pub(crate) fn keep(&mut self) {
        self.state = AdoptionState::Kept;
    }
}
