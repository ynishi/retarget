//! The places of levers in a customer's files, and the guard on writing them,
//! shared by the Lab's ledger and the product's portfolio.
//!
//! The rule is stated in [`crate::domain::put_back`]. This module holds the
//! state it needs and takes every decision about writing a place, so that the
//! two layers share one copy: each wraps a [`PlaceEvent`] in its own events and
//! decides only what happens to its experiment or adoption when a person's
//! edit is found.
//!
//! ```text
//! prepare_write(current, restore) ── Proceed ──> WriteApproved ──> app writes ──> written ──> Written
//!                                  └─ ExternalEdit: the owner stops, and ExternalEditFound blocks the place
//! a write that lands after its owner closed ──> Written + PutBackDue(restore)
//! PutBackDue ──> check_put_back (same rule) ──> PutBackApproved ──> app writes ──> PutBackDone
//! blocked ──> resolve (a person accepts their edit) ──> ExternalEditResolved
//! ```
//!
//! # Invariants
//!
//! - A write is recorded only after it was approved; a put-back only after it
//!   was approved.
//! - A write approved for an owner that has since closed is still recorded
//!   when it lands, and is put back at once: the files never keep a change no
//!   owner answers for.
//! - A place someone edited stays blocked until a person resolves it.
//! - A change is due to be put back only if retarget wrote the place.
//!
//! # Errors
//!
//! [`PlacesError`] names the step that was skipped; send the step it names first.

use std::collections::BTreeMap;

use crate::domain::name::{Digest, LeverKey, PositionId};
use crate::domain::put_back::{Baseline, PutBack, check_write};

/// A step of writing or putting back a lever's place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceEvent {
    /// A write may go ahead; the place held `current`, and `restore` is where
    /// it goes back to.
    WriteApproved {
        /// The lever.
        lever: LeverKey,
        /// What the place held when the write was approved.
        current: Option<Digest>,
        /// Where the place goes back to.
        restore: PositionId,
    },
    /// The place now holds what retarget wrote.
    Written(LeverKey, Digest),
    /// Someone edited the place; it is blocked, to go back to this position once resolved.
    ExternalEditFound(LeverKey, PositionId),
    /// The place has to be put back to this position.
    PutBackDue(LeverKey, PositionId),
    /// Putting the place back may go ahead.
    PutBackApproved(LeverKey),
    /// The place was put back.
    PutBackDone(LeverKey),
    /// A person accepted their edit as the place's contents.
    ExternalEditResolved(LeverKey),
}

/// Why a step of writing was refused, or a place event could not be folded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlacesError {
    /// The write was not approved first.
    #[error("the write of lever `{0}` was not approved")]
    NotApproved(LeverKey),
    /// The lever has nothing to put back.
    #[error("lever `{0}` has nothing to put back")]
    NothingToPutBack(LeverKey),
    /// The lever is not blocked by an edit.
    #[error("lever `{0}` is not blocked")]
    NotBlocked(LeverKey),
    /// A place event does not fit the state it is folded into.
    #[error("a place event does not fit: {0}")]
    Unfoldable(&'static str),
}

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
    Write(PositionId),
    PutBack,
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
    /// Whether retarget wrote the place.
    pub fn wrote(&self, lever: &LeverKey) -> bool {
        self.places.get(lever).is_some_and(|p| p.wrote)
    }
    /// Whether the place still has a change to put back or is blocked.
    pub fn is_pending(&self, lever: &LeverKey) -> bool {
        self.put_back.contains_key(lever)
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

    /// Whether a write of the place may go ahead, given what it holds now.
    /// A place never seen before takes what it holds now as what it held first.
    pub fn prepare_write(
        &self,
        lever: &LeverKey,
        current: Option<Digest>,
        restore: PositionId,
    ) -> Result<PlaceEvent, PutBack> {
        let verdict = match self.places.get(lever) {
            Some(place) => check_write(&place.expected, current.as_ref()),
            None => PutBack::Proceed,
        };
        match verdict {
            PutBack::Proceed => Ok(PlaceEvent::WriteApproved {
                lever: lever.clone(),
                current,
                restore,
            }),
            PutBack::ExternalEdit => Err(PutBack::ExternalEdit),
        }
    }
    /// The place was written. `owned` says whether an experiment or adoption
    /// still answers for the lever; when none does, the change goes back at once.
    pub fn written(
        &self,
        lever: &LeverKey,
        digest: Digest,
        owned: bool,
    ) -> Result<Vec<PlaceEvent>, PlacesError> {
        let Some(Approval::Write(restore)) = self.approved.get(lever) else {
            return Err(PlacesError::NotApproved(lever.clone()));
        };
        let mut events = vec![PlaceEvent::Written(lever.clone(), digest)];
        if !owned {
            events.push(PlaceEvent::PutBackDue(lever.clone(), restore.clone()));
        }
        Ok(events)
    }
    /// The owner closed: the change goes back if retarget wrote the place.
    pub fn closed(&self, lever: &LeverKey, restore: &PositionId) -> Option<PlaceEvent> {
        self.wrote(lever)
            .then(|| PlaceEvent::PutBackDue(lever.clone(), restore.clone()))
    }
    /// About to put the place back; `current` is what it holds.
    pub fn check_put_back(
        &self,
        lever: &LeverKey,
        current: Option<Digest>,
    ) -> Result<PlaceEvent, PlacesError> {
        let Some(PutBackState::Due(to)) = self.put_back.get(lever) else {
            return Err(PlacesError::NothingToPutBack(lever.clone()));
        };
        let expected = self.places.get(lever).map(|p| p.expected);
        let verdict = match expected {
            Some(expected) => check_write(&expected, current.as_ref()),
            None => PutBack::Proceed,
        };
        Ok(match verdict {
            PutBack::Proceed => PlaceEvent::PutBackApproved(lever.clone()),
            PutBack::ExternalEdit => PlaceEvent::ExternalEditFound(lever.clone(), to.clone()),
        })
    }
    /// The place was put back.
    pub fn put_back_written(&self, lever: &LeverKey) -> Result<PlaceEvent, PlacesError> {
        match self.approved.get(lever) {
            Some(Approval::PutBack) => Ok(PlaceEvent::PutBackDone(lever.clone())),
            _ => Err(PlacesError::NotApproved(lever.clone())),
        }
    }
    /// A person accepts their edit.
    pub fn resolve(&self, lever: &LeverKey) -> Result<PlaceEvent, PlacesError> {
        match self.put_back.get(lever) {
            Some(PutBackState::Blocked(_)) => Ok(PlaceEvent::ExternalEditResolved(lever.clone())),
            _ => Err(PlacesError::NotBlocked(lever.clone())),
        }
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

    /// Fold a place event into the state; nothing is decided here.
    pub(crate) fn apply(&mut self, event: &PlaceEvent) -> Result<(), PlacesError> {
        match event {
            PlaceEvent::WriteApproved {
                lever,
                current,
                restore,
            } => {
                self.places.entry(lever.clone()).or_insert(Place {
                    expected: current.map_or(Baseline::Empty, Baseline::Held),
                    wrote: false,
                });
                self.approved
                    .insert(lever.clone(), Approval::Write(restore.clone()));
            }
            PlaceEvent::Written(lever, digest) => {
                if !matches!(self.approved.remove(lever), Some(Approval::Write(_))) {
                    return Err(PlacesError::Unfoldable("a write that was not approved"));
                }
                let place = self
                    .places
                    .get_mut(lever)
                    .ok_or(PlacesError::Unfoldable("a written place was never seen"))?;
                place.expected = Baseline::Held(*digest);
                place.wrote = true;
            }
            PlaceEvent::ExternalEditFound(lever, to) => {
                self.approved.remove(lever);
                self.put_back
                    .insert(lever.clone(), PutBackState::Blocked(to.clone()));
            }
            PlaceEvent::PutBackDue(lever, to) => {
                self.put_back
                    .insert(lever.clone(), PutBackState::Due(to.clone()));
            }
            PlaceEvent::PutBackApproved(lever) => {
                if !matches!(self.put_back.get(lever), Some(PutBackState::Due(_))) {
                    return Err(PlacesError::Unfoldable("nothing due to put back"));
                }
                self.approved.insert(lever.clone(), Approval::PutBack);
            }
            PlaceEvent::PutBackDone(lever) | PlaceEvent::ExternalEditResolved(lever) => {
                self.approved.remove(lever);
                self.put_back.remove(lever);
                self.places.remove(lever);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lever() -> LeverKey {
        "l".parse().unwrap()
    }
    fn at(s: &str) -> PositionId {
        s.parse().unwrap()
    }

    #[test]
    fn a_write_that_lands_after_its_owner_closed_goes_back_at_once() {
        let mut places = Places::default();
        let approved = places.prepare_write(&lever(), None, at("a")).unwrap();
        places.apply(&approved).unwrap();
        let events = places
            .written(&lever(), Digest::new([1; 32]), false)
            .unwrap();
        assert_eq!(
            events,
            vec![
                PlaceEvent::Written(lever(), Digest::new([1; 32])),
                PlaceEvent::PutBackDue(lever(), at("a")),
            ]
        );
        for e in &events {
            places.apply(e).unwrap();
        }
        assert_eq!(places.pending_put_back(), vec![(&lever(), &at("a"))]);
    }

    #[test]
    fn an_edit_blocks_and_only_a_person_unblocks() {
        let mut places = Places::default();
        places
            .apply(&places.prepare_write(&lever(), None, at("a")).unwrap())
            .unwrap();
        for e in places
            .written(&lever(), Digest::new([1; 32]), true)
            .unwrap()
        {
            places.apply(&e).unwrap();
        }
        assert_eq!(
            places.prepare_write(&lever(), Some(Digest::new([9; 32])), at("a")),
            Err(PutBack::ExternalEdit)
        );
        places
            .apply(&PlaceEvent::ExternalEditFound(lever(), at("a")))
            .unwrap();
        assert_eq!(places.blocked(), vec![&lever()]);
        assert!(places.check_put_back(&lever(), None).is_err());
        places.apply(&places.resolve(&lever()).unwrap()).unwrap();
        assert!(places.blocked().is_empty());
    }
}
