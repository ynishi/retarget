//! The places of levers in a customer's files, and the guard on writing them,
//! shared by the Lab's ledger and the product's portfolio.
//!
//! The rule is stated in [`crate::domain::put_back`]; this module keeps the
//! state it needs: what each place is expected to hold, which write was
//! approved, and what must still be put back.
//!
//! ```text
//! check_write(expected, current) ── Proceed ──> approve_write ──> app writes ──> written(digest)
//!                                └─ ExternalEdit ──> external_edit: blocked, nothing written
//! put_back_due ──> check (same rule) ──> approve_put_back ──> app writes ──> put_back_done
//! blocked ──> resolved (a person accepts their edit)
//! ```
//!
//! # Invariants
//!
//! - A write is recorded only after it was approved; a put-back only after it
//!   was approved.
//! - A place someone edited stays blocked until a person resolves it.
//! - A change is due to be put back only if retarget wrote the place.

use std::collections::BTreeMap;

use crate::domain::name::{Digest, LeverKey, PositionId};
use crate::domain::put_back::{Baseline, PutBack, check_write};

/// A change that has to be put back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutBackState {
    /// It may be checked and put back to this position.
    Due(PositionId),
    /// Someone edited the place; it is left as they left it.
    Blocked(PositionId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Approval {
    Write,
    PutBack(PositionId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    expected: Baseline,
    wrote: bool,
}

/// The places of the levers of one scope.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Places {
    places: BTreeMap<LeverKey, Place>,
    approved: BTreeMap<LeverKey, Approval>,
    put_back: BTreeMap<LeverKey, PutBackState>,
}

impl Places {
    /// Whether writing a lever's place may go ahead, given what it holds now.
    /// A place never seen before takes what it holds now as what it held first.
    pub fn check(&self, lever: &LeverKey, current: Option<&Digest>) -> PutBack {
        match self.places.get(lever) {
            Some(place) => check_write(&place.expected, current),
            None => PutBack::Proceed,
        }
    }
    /// Whether a write of the place was approved.
    pub fn write_approved(&self, lever: &LeverKey) -> bool {
        matches!(self.approved.get(lever), Some(Approval::Write))
    }
    /// Whether putting the place back was approved.
    pub fn put_back_approved(&self, lever: &LeverKey) -> bool {
        matches!(self.approved.get(lever), Some(Approval::PutBack(_)))
    }
    /// Where the place is due to be put back to, if it is.
    pub fn due(&self, lever: &LeverKey) -> Option<&PositionId> {
        match self.put_back.get(lever) {
            Some(PutBackState::Due(to)) => Some(to),
            _ => None,
        }
    }
    /// Whether the place is blocked by a person's edit.
    pub fn is_blocked(&self, lever: &LeverKey) -> bool {
        matches!(self.put_back.get(lever), Some(PutBackState::Blocked(_)))
    }
    /// Whether the place still has a change to put back or is blocked.
    pub fn is_pending(&self, lever: &LeverKey) -> bool {
        self.put_back.contains_key(lever)
    }
    /// Whether retarget wrote the place.
    pub fn wrote(&self, lever: &LeverKey) -> bool {
        self.places.get(lever).is_some_and(|p| p.wrote)
    }
    /// The changes to put back, and where to.
    pub fn pending_put_back(&self) -> Vec<(&LeverKey, &PositionId)> {
        self.put_back
            .iter()
            .filter_map(|(lever, state)| match state {
                PutBackState::Due(to) => Some((lever, to)),
                PutBackState::Blocked(_) => None,
            })
            .collect()
    }
    /// The places left as a person edited them.
    pub fn blocked(&self) -> Vec<&LeverKey> {
        self.put_back
            .iter()
            .filter(|(_, state)| matches!(state, PutBackState::Blocked(_)))
            .map(|(lever, _)| lever)
            .collect()
    }

    /// Record what a place held when retarget started with it.
    pub(crate) fn record_baseline(&mut self, lever: &LeverKey, baseline: Baseline) {
        self.places.insert(
            lever.clone(),
            Place {
                expected: baseline,
                wrote: false,
            },
        );
    }
    /// A write was approved; a place never seen before keeps `current` as
    /// what it held first.
    pub(crate) fn approve_write(&mut self, lever: &LeverKey, current: Option<Digest>) {
        self.places.entry(lever.clone()).or_insert(Place {
            expected: current.map_or(Baseline::Empty, Baseline::Held),
            wrote: false,
        });
        self.approved.insert(lever.clone(), Approval::Write);
    }
    /// The place now holds what retarget wrote.
    pub(crate) fn written(&mut self, lever: &LeverKey, digest: Digest) -> bool {
        if self.approved.remove(lever).is_none() {
            return false;
        }
        match self.places.get_mut(lever) {
            Some(place) => {
                place.expected = Baseline::Held(digest);
                place.wrote = true;
                true
            }
            None => false,
        }
    }
    /// Someone edited the place: it is blocked, to be put back to `to` once resolved.
    pub(crate) fn external_edit(&mut self, lever: &LeverKey, to: PositionId) {
        self.approved.remove(lever);
        self.put_back
            .insert(lever.clone(), PutBackState::Blocked(to));
    }
    /// The place has to be put back to `to`.
    pub(crate) fn put_back_due(&mut self, lever: &LeverKey, to: PositionId) {
        self.put_back.insert(lever.clone(), PutBackState::Due(to));
    }
    /// Putting the place back was approved.
    pub(crate) fn approve_put_back(&mut self, lever: &LeverKey) -> bool {
        match self.put_back.get(lever) {
            Some(PutBackState::Due(to)) => {
                self.approved
                    .insert(lever.clone(), Approval::PutBack(to.clone()));
                true
            }
            _ => false,
        }
    }
    /// The place was put back, or a person accepted their edit: retarget
    /// forgets it until it starts with it again.
    pub(crate) fn forget(&mut self, lever: &LeverKey) {
        self.approved.remove(lever);
        self.put_back.remove(lever);
        self.places.remove(lever);
    }
}
