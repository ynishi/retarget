//! The shape of a lever: the positions it can take, where it is written, the
//! role it is scoped to, and the mechanisms it claims to act on.
//!
//! A lever arrives from an adapter's catalogue as a value. What it is called in
//! a harness, and which key of which file holds it, stay in the adapter; the
//! domain judges a lever by its shape alone, so adding a lever changes the
//! catalogue and not this module.
//!
//! # Invariants
//!
//! - A lever has at least two positions and no position twice.
//! - It claims at least one mechanism, and each claim names a move its kind of
//!   positions can make: up or down for ordered positions, toward a position
//!   for unordered ones.
//! - One step goes to a neighbour (ordered) or to another listed position
//!   (unordered), decided by [`Lever::apply`] alone.
//!
//! # Errors
//!
//! [`LeverError`] names the rule a catalogue entry or a step broke; fix the
//! catalogue entry, or choose a step the lever can make.

use std::collections::BTreeSet;

use crate::domain::mechanism::Mechanism;
use crate::domain::name::{LeverKey, PositionId, RoleKey};

/// Whether a lever's positions have an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Ordered: a step goes up or down by one.
    Ordinal,
    /// Unordered: a step goes to any other position.
    Nominal,
}

/// The positions a lever can take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Positions {
    order: Order,
    items: Vec<PositionId>,
}

impl Positions {
    /// The positions, refused when there are fewer than two or one repeats.
    pub fn new(order: Order, items: Vec<PositionId>) -> Result<Self, LeverError> {
        if items.len() < 2 {
            return Err(LeverError::NotEnoughPositions);
        }
        let mut seen = BTreeSet::new();
        for item in &items {
            if !seen.insert(item) {
                return Err(LeverError::DuplicatePosition(item.clone()));
            }
        }
        Ok(Self { order, items })
    }
    /// Whether they are ordered.
    pub fn order(&self) -> Order {
        self.order
    }
    /// The positions, in order.
    pub fn items(&self) -> &[PositionId] {
        &self.items
    }
    /// Whether a position is one of them.
    pub fn contains(&self, position: &PositionId) -> bool {
        self.items.contains(position)
    }
    fn index(&self, position: &PositionId) -> Result<usize, LeverError> {
        self.items
            .iter()
            .position(|item| item == position)
            .ok_or_else(|| LeverError::UnknownPosition(position.clone()))
    }
}

/// A direction along ordered positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// To the next position.
    Up,
    /// To the previous position.
    Down,
}

/// One step of a lever, the unit of an experiment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// One position up or down (ordered positions).
    Ordinal(Direction),
    /// To another position (unordered positions).
    Nominal(PositionId),
}

/// The move under which a claimed mechanism is reduced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimWhen {
    /// Moving up (ordered positions).
    Up,
    /// Moving down (ordered positions).
    Down,
    /// Moving to this position (unordered positions).
    Toward(PositionId),
}

impl ClaimWhen {
    /// Whether a step is the move this claim names.
    pub fn matches(&self, step: &Step) -> bool {
        match (self, step) {
            (ClaimWhen::Up, Step::Ordinal(Direction::Up))
            | (ClaimWhen::Down, Step::Ordinal(Direction::Down)) => true,
            (ClaimWhen::Toward(a), Step::Nominal(b)) => a == b,
            _ => false,
        }
    }
}

/// A lever's claim that moving it one way reduces a mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MechanismClaim {
    /// The mechanism.
    pub mechanism: Mechanism,
    /// The move that reduces it.
    pub reduced_when: ClaimWhen,
}

/// Where a lever is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The harness's project settings.
    ProjectSettings,
    /// The project's agent definitions.
    ProjectAgentDef,
    /// The project's instructions.
    ProjectInstruction,
    /// The user's own scope.
    UserScope,
}

/// Whether retarget may write a surface or only show the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applicability {
    /// retarget writes it.
    Apply,
    /// retarget shows it to the user, who decides.
    ShowOnly,
}

impl Surface {
    /// Whether retarget may write this surface: never the user's own scope.
    pub fn applicability(self) -> Applicability {
        match self {
            Surface::UserScope => Applicability::ShowOnly,
            Surface::ProjectSettings | Surface::ProjectAgentDef | Surface::ProjectInstruction => {
                Applicability::Apply
            }
        }
    }
}

/// Which runs a lever acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleScope {
    /// Every run.
    All,
    /// Runs in which a subagent of this role works.
    Role(RoleKey),
}

/// Why a lever or a step was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LeverError {
    /// A lever needs at least two positions.
    #[error("a lever needs at least two positions")]
    NotEnoughPositions,
    /// A position appears twice.
    #[error("position `{0}` appears twice")]
    DuplicatePosition(PositionId),
    /// A lever must claim at least one mechanism.
    #[error("a lever must claim at least one mechanism")]
    NoClaims,
    /// A claim names a move the positions cannot make.
    #[error("a claim names a move its positions cannot make")]
    ClaimMismatch,
    /// A position is not one of the lever's.
    #[error("position `{0}` is not one of the lever's")]
    UnknownPosition(PositionId),
    /// The step would leave the ordered positions.
    #[error("the step goes past the last position")]
    AtEnd,
    /// The kind of step does not fit the kind of positions.
    #[error("the step does not fit the lever's kind of positions")]
    StepMismatch,
    /// An unordered step to where the lever already is.
    #[error("the step goes nowhere")]
    SameStep,
}

/// One thing about how the agent runs that retarget may change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lever {
    key: LeverKey,
    positions: Positions,
    surface: Surface,
    scope: RoleScope,
    claims: Vec<MechanismClaim>,
}

impl Lever {
    /// A lever, refused when its claims do not fit it.
    pub fn new(
        key: LeverKey,
        positions: Positions,
        surface: Surface,
        scope: RoleScope,
        claims: Vec<MechanismClaim>,
    ) -> Result<Self, LeverError> {
        if claims.is_empty() {
            return Err(LeverError::NoClaims);
        }
        for claim in &claims {
            let fits = match (&claim.reduced_when, positions.order()) {
                (ClaimWhen::Up | ClaimWhen::Down, Order::Ordinal) => true,
                (ClaimWhen::Toward(to), Order::Nominal) => positions.contains(to),
                _ => false,
            };
            if !fits {
                return Err(LeverError::ClaimMismatch);
            }
        }
        Ok(Self {
            key,
            positions,
            surface,
            scope,
            claims,
        })
    }
    /// Its identity.
    pub fn key(&self) -> &LeverKey {
        &self.key
    }
    /// Its positions.
    pub fn positions(&self) -> &Positions {
        &self.positions
    }
    /// Where it is written.
    pub fn surface(&self) -> Surface {
        self.surface
    }
    /// Which runs it acts on.
    pub fn scope(&self) -> &RoleScope {
        &self.scope
    }
    /// The mechanisms it claims to act on.
    pub fn claims(&self) -> &[MechanismClaim] {
        &self.claims
    }
    /// Where one step from a position leads.
    pub fn apply(&self, from: &PositionId, step: &Step) -> Result<PositionId, LeverError> {
        let at = self.positions.index(from)?;
        match (step, self.positions.order()) {
            (Step::Ordinal(direction), Order::Ordinal) => {
                let to = match direction {
                    Direction::Up => at + 1,
                    Direction::Down => at.checked_sub(1).ok_or(LeverError::AtEnd)?,
                };
                self.positions
                    .items
                    .get(to)
                    .cloned()
                    .ok_or(LeverError::AtEnd)
            }
            (Step::Nominal(to), Order::Nominal) => {
                self.positions.index(to)?;
                if to == from {
                    Err(LeverError::SameStep)
                } else {
                    Ok(to.clone())
                }
            }
            _ => Err(LeverError::StepMismatch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<PositionId> {
        names.iter().map(|n| n.parse().unwrap()).collect()
    }

    fn ttl() -> Lever {
        Lever::new(
            "cache.off_main_ttl".parse().unwrap(),
            Positions::new(Order::Ordinal, ids(&["5m", "1h"])).unwrap(),
            Surface::ProjectSettings,
            RoleScope::All,
            vec![MechanismClaim {
                mechanism: Mechanism::CacheExpiry,
                reduced_when: ClaimWhen::Up,
            }],
        )
        .unwrap()
    }

    #[test]
    fn positions_need_two_distinct() {
        assert_eq!(
            Positions::new(Order::Ordinal, ids(&["a"])),
            Err(LeverError::NotEnoughPositions)
        );
        assert!(matches!(
            Positions::new(Order::Ordinal, ids(&["a", "a"])),
            Err(LeverError::DuplicatePosition(_))
        ));
    }

    #[test]
    fn claims_must_fit_the_positions() {
        let positions = Positions::new(Order::Nominal, ids(&["a", "b"])).unwrap();
        let claim = |when| MechanismClaim {
            mechanism: Mechanism::ModelPrice,
            reduced_when: when,
        };
        let lever = |claims| {
            Lever::new(
                "l".parse().unwrap(),
                positions.clone(),
                Surface::ProjectSettings,
                RoleScope::All,
                claims,
            )
        };
        assert_eq!(lever(vec![]), Err(LeverError::NoClaims));
        assert_eq!(
            lever(vec![claim(ClaimWhen::Up)]),
            Err(LeverError::ClaimMismatch)
        );
        assert_eq!(
            lever(vec![claim(ClaimWhen::Toward("c".parse().unwrap()))]),
            Err(LeverError::ClaimMismatch)
        );
        assert!(lever(vec![claim(ClaimWhen::Toward("b".parse().unwrap()))]).is_ok());
    }

    #[test]
    fn a_step_goes_to_a_neighbour_or_fails() {
        let lever = ttl();
        let five = "5m".parse().unwrap();
        let hour: PositionId = "1h".parse().unwrap();
        assert_eq!(
            lever.apply(&five, &Step::Ordinal(Direction::Up)),
            Ok(hour.clone())
        );
        assert_eq!(
            lever.apply(&hour, &Step::Ordinal(Direction::Up)),
            Err(LeverError::AtEnd)
        );
        assert_eq!(
            lever.apply(&five, &Step::Ordinal(Direction::Down)),
            Err(LeverError::AtEnd)
        );
        assert_eq!(
            lever.apply(&five, &Step::Nominal(hour)),
            Err(LeverError::StepMismatch)
        );
    }

    #[test]
    fn the_user_scope_is_shown_not_written() {
        assert_eq!(Surface::UserScope.applicability(), Applicability::ShowOnly);
        assert_eq!(
            Surface::ProjectSettings.applicability(),
            Applicability::Apply
        );
    }
}
