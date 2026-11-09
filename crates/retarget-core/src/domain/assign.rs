//! Windows of runs and the arms runs are assigned to, shared by the Lab and
//! the product.
//!
//! A window is the set of runs assigned to two positions of one lever with one
//! probability. That probability is fixed when the window opens and kept here
//! only, because every comparison weights runs by it.
//!
//! # Invariants
//!
//! - The control and treatment positions of a window differ.
//! - Windows are made only by the layers that own them (the Lab's ledger, the
//!   product's portfolio), so their kind and probability follow from a decision.

use std::collections::BTreeMap;

use crate::domain::lever::RoleScope;
use crate::domain::name::{AdoptionId, ExperimentId, LeverKey, PositionId, Ratio, WindowId};
use crate::domain::run::{Run, TaskClass};

/// The two arms of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    /// The position compared against.
    Control,
    /// The position under test.
    Treatment,
}

/// What a window is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowKind {
    /// The Lab's trial of an experiment.
    Trial(ExperimentId),
    /// The Lab's confirmation of a provisional keep against the holdout.
    Confirm(ExperimentId),
    /// The Lab's watch of a kept lever against the holdout.
    Watch(LeverKey),
    /// The product's check that an adopted technique fits.
    Adoption(AdoptionId),
}

/// The runs assigned with one probability, and what their arms mean.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    id: WindowId,
    kind: WindowKind,
    lever: LeverKey,
    control: PositionId,
    treatment: PositionId,
    p: Ratio,
    task_class: Option<TaskClass>,
    scope: RoleScope,
}

/// The parts of a window, given by the layer that opens it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowParts {
    pub id: WindowId,
    pub kind: WindowKind,
    pub lever: LeverKey,
    pub control: PositionId,
    pub treatment: PositionId,
    pub p: Ratio,
    pub task_class: Option<TaskClass>,
    pub scope: RoleScope,
}

impl Window {
    /// A window. The positions are distinct by construction of the step that
    /// produced them; a window that would compare a position with itself is a
    /// bug of the caller.
    pub(crate) fn new(parts: WindowParts) -> Self {
        debug_assert!(
            parts.control != parts.treatment,
            "a window compares two positions"
        );
        Self {
            id: parts.id,
            kind: parts.kind,
            lever: parts.lever,
            control: parts.control,
            treatment: parts.treatment,
            p: parts.p,
            task_class: parts.task_class,
            scope: parts.scope,
        }
    }
    /// Its id.
    pub fn id(&self) -> WindowId {
        self.id
    }
    /// What it is for.
    pub fn kind(&self) -> &WindowKind {
        &self.kind
    }
    /// The lever it assigns.
    pub fn lever(&self) -> &LeverKey {
        &self.lever
    }
    /// The position of the control arm.
    pub fn control(&self) -> &PositionId {
        &self.control
    }
    /// The position of the treatment arm.
    pub fn treatment(&self) -> &PositionId {
        &self.treatment
    }
    /// The probability of the treatment arm.
    pub fn p(&self) -> f64 {
        self.p.get()
    }
    /// The task class its runs must be in; any class when there is none.
    pub fn task_class(&self) -> Option<&TaskClass> {
        self.task_class.as_ref()
    }
    /// Which runs the lever acts on.
    pub fn scope(&self) -> &RoleScope {
        &self.scope
    }
    /// The arm a position belongs to, if any.
    pub fn arm_of(&self, position: &PositionId) -> Option<Arm> {
        if position == &self.treatment {
            Some(Arm::Treatment)
        } else if position == &self.control {
            Some(Arm::Control)
        } else {
            None
        }
    }
}

/// The window and position a run was assigned to for one lever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assigned {
    /// The window.
    pub window: WindowId,
    /// The position written for the run.
    pub position: PositionId,
}

/// A run and what it was assigned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedRun {
    /// The run.
    pub run: Run,
    /// Its assignments, per lever.
    pub assigned: BTreeMap<LeverKey, Assigned>,
}
