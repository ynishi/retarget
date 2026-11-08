//! A run's cost, split into cost terms, and the cost the objective counts.
//!
//! A cost term is one cell of a run's bill: the part of the agent it was spent
//! in ([`Segment`]), the kind of token ([`TokenKind`]), and whether it was spent
//! recovering from a failure ([`Work`]). A [`TermSet`] is a set of cells, so a
//! cell is never counted twice however sets are combined.
//!
//! ```text
//! Run ──> CostBreakdown::of
//!          ├─ cells: (segment, kind, work) -> cost   (only when every request was priced)
//!          └─ objective: reported total, or the priced parts where none was reported
//! ```
//!
//! # Invariants
//!
//! - Which requests are rework, which cells hold what, and what the objective
//!   counts are decided here and nowhere else.
//! - A request is rework when, in the same node, it comes after a retry, an API
//!   error or a failed tool call, and before the next tool call that worked.
//! - The cells exist only when every request of the run has its priced parts;
//!   otherwise they are missing, and only measures of cost terms leave the run
//!   out.
//!
//! # Errors
//!
//! - [`CostError::NoRequests`]: the run made no model request; it did no work
//!   and is not counted (it would be a free success).
//! - [`CostError::Unknown`]: a request has neither a reported cost nor priced
//!   parts, so the run cannot be counted (the adapter sent too little).
//! - [`CostError::Overflow`]: a sum did not fit (a broken input).

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::money::{Cost, MoneyError, TokenKind};
use crate::domain::run::{AgentNode, Element, Incident, Observed, Role, Run, ToolResult};

/// The part of the agent a cost was spent in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Segment {
    /// The main conversation.
    Main,
    /// Subagents and auxiliary work.
    OffMain,
}

/// Whether a cost moved the task forward or recovered from a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Work {
    /// Work that moved the task forward.
    First,
    /// Work spent recovering from a failure.
    Rework,
}

/// One cell of a run's bill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cell {
    /// Where it was spent.
    pub segment: Segment,
    /// On which kind of token.
    pub kind: TokenKind,
    /// On which kind of work.
    pub work: Work,
}

/// A set of cells.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TermSet(BTreeSet<Cell>);

impl TermSet {
    /// No cell.
    pub fn empty() -> Self {
        Self::default()
    }
    /// Every cell.
    pub fn all() -> Self {
        let mut cells = BTreeSet::new();
        for segment in [Segment::Main, Segment::OffMain] {
            for kind in TokenKind::ALL {
                for work in [Work::First, Work::Rework] {
                    cells.insert(Cell {
                        segment,
                        kind,
                        work,
                    });
                }
            }
        }
        Self(cells)
    }
    /// The cells of one kind of token.
    pub fn of_kind(kind: TokenKind) -> Self {
        Self::all().filter(|cell| cell.kind == kind)
    }
    /// The cells of one segment.
    pub fn of_segment(segment: Segment) -> Self {
        Self::all().filter(|cell| cell.segment == segment)
    }
    /// The cells of one kind of work.
    pub fn of_work(work: Work) -> Self {
        Self::all().filter(|cell| cell.work == work)
    }
    fn filter(self, keep: impl Fn(&Cell) -> bool) -> Self {
        Self(self.0.into_iter().filter(|cell| keep(cell)).collect())
    }
    /// The cells in either set.
    pub fn union(&self, other: &TermSet) -> TermSet {
        Self(self.0.union(&other.0).copied().collect())
    }
    /// The cells in both sets.
    pub fn intersection(&self, other: &TermSet) -> TermSet {
        Self(self.0.intersection(&other.0).copied().collect())
    }
    /// The cells in this set and not the other.
    pub fn difference(&self, other: &TermSet) -> TermSet {
        Self(self.0.difference(&other.0).copied().collect())
    }
    /// Whether the two sets share no cell.
    pub fn is_disjoint(&self, other: &TermSet) -> bool {
        self.0.is_disjoint(&other.0)
    }
    /// Whether the set has no cell.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Whether the set holds a cell.
    pub fn contains(&self, cell: &Cell) -> bool {
        self.0.contains(cell)
    }
}

/// Where the objective's cost of a run came from, so that arms whose costs
/// were taken differently are noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostOrigin {
    /// Every request's cost was reported by the harness.
    Reported,
    /// Every request's cost was priced from its tokens.
    Priced,
    /// Some of each.
    Mixed,
}

/// The cost the objective counts for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectiveCost {
    /// The cost.
    pub cost: Cost,
    /// Where it came from.
    pub origin: CostOrigin,
}

/// Why a run's cost could not be split or counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CostError {
    /// A request has neither a reported cost nor priced parts.
    #[error("a request has neither a reported cost nor priced parts")]
    Unknown,
    /// The run made no model request, so there is nothing to count.
    #[error("the run made no model request")]
    NoRequests,
    /// A sum did not fit.
    #[error(transparent)]
    Overflow(#[from] MoneyError),
}

/// A run's cost, by cell and for the objective.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostBreakdown {
    cells: Observed<BTreeMap<Cell, Cost>>,
    objective: ObjectiveCost,
}

#[derive(Default)]
struct Tally {
    cells: BTreeMap<Cell, Cost>,
    cells_complete: bool,
    cells_total: Cost,
    objective: Cost,
    reported: usize,
    priced: usize,
}

impl CostBreakdown {
    /// Split a run's cost. This is the one place that decides which requests
    /// are rework and what the objective counts.
    pub fn of(run: &Run) -> Result<Self, CostError> {
        let mut tally = Tally {
            cells_complete: true,
            ..Tally::default()
        };
        walk(run.root(), &mut tally)?;
        let origin = match (tally.reported, tally.priced) {
            // a run with no request cost nothing and did no work; counting it
            // would be a free success for whichever arm it landed in
            (0, 0) => return Err(CostError::NoRequests),
            (_, 0) => CostOrigin::Reported,
            (0, _) => CostOrigin::Priced,
            _ => CostOrigin::Mixed,
        };
        Ok(Self {
            cells: if tally.cells_complete {
                Observed::Known(tally.cells)
            } else {
                Observed::Missing
            },
            objective: ObjectiveCost {
                cost: tally.objective,
                origin,
            },
        })
    }
    /// The cost the objective counts.
    pub fn objective(&self) -> ObjectiveCost {
        self.objective
    }
    /// Whether the cells are known (every request was priced).
    pub fn has_terms(&self) -> bool {
        matches!(self.cells, Observed::Known(_))
    }
    /// The sum of a set of cells, missing when the cells are. It cannot
    /// overflow: the total of all cells was checked when they were built.
    pub fn sum(&self, set: &TermSet) -> Observed<Cost> {
        match &self.cells {
            Observed::Known(cells) => Observed::Known(Cost::from_micros(
                cells
                    .iter()
                    .filter(|(cell, _)| set.contains(cell))
                    .map(|(_, cost)| cost.micros())
                    .sum(),
            )),
            Observed::Missing => Observed::Missing,
        }
    }
}

fn walk(node: &AgentNode, tally: &mut Tally) -> Result<(), CostError> {
    let segment = match node.role() {
        Role::Main => Segment::Main,
        Role::Sub(_) | Role::Auxiliary(_) => Segment::OffMain,
    };
    let mut recovering = false;
    for element in node.elements() {
        match element {
            Element::Incident(Incident::Retry | Incident::ApiError) => recovering = true,
            Element::Incident(_) => {}
            Element::ToolCall(call) => recovering = call.result == ToolResult::Err,
            Element::Request(request) => {
                let work = if recovering {
                    Work::Rework
                } else {
                    Work::First
                };
                match (&request.cost.reported, &request.cost.terms) {
                    (Observed::Known(cost), _) => {
                        tally.objective = tally.objective.checked_add(*cost)?;
                        tally.reported += 1;
                    }
                    (Observed::Missing, Observed::Known(terms)) => {
                        tally.objective = tally.objective.checked_add(terms.total()?)?;
                        tally.priced += 1;
                    }
                    (Observed::Missing, Observed::Missing) => return Err(CostError::Unknown),
                }
                match &request.cost.terms {
                    Observed::Known(terms) => {
                        for kind in TokenKind::ALL {
                            let cell = Cell {
                                segment,
                                kind,
                                work,
                            };
                            let part = terms.get(kind);
                            tally.cells_total = tally.cells_total.checked_add(part)?;
                            let slot = tally.cells.entry(cell).or_insert(Cost::ZERO);
                            *slot = slot.checked_add(part)?;
                        }
                    }
                    Observed::Missing => tally.cells_complete = false,
                }
            }
        }
    }
    for child in node.children() {
        walk(child, tally)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::run::{AgentNode, Role, ToolCall, ToolClass};
    use crate::domain::testkit;

    fn cell(segment: Segment, kind: TokenKind, work: Work) -> TermSet {
        TermSet::of_segment(segment)
            .intersection(&TermSet::of_kind(kind))
            .intersection(&TermSet::of_work(work))
    }

    #[test]
    fn rework_runs_from_a_failure_to_the_next_tool_that_worked() {
        let req = || Element::Request(testkit::request(1, 0));
        let elements = vec![
            req(),
            Element::Incident(Incident::Retry),
            req(),
            req(),
            Element::ToolCall(ToolCall {
                class: ToolClass::Exec,
                result: ToolResult::Ok,
            }),
            req(),
        ];
        let child = AgentNode::new(Role::Sub("r".parse().unwrap()), vec![req()], vec![]);
        let root = AgentNode::new(Role::Main, elements, vec![child]);
        let mut parts = testkit::run_parts(Outcome::Success, vec![]);
        parts.root = root;
        let run = Run::new(parts).unwrap();
        let split = CostBreakdown::of(&run).unwrap();
        let micros = |set: TermSet| split.sum(&set).known().unwrap().micros();
        assert_eq!(
            micros(cell(Segment::Main, TokenKind::CacheWrite, Work::Rework)),
            2
        );
        assert_eq!(
            micros(cell(Segment::Main, TokenKind::CacheWrite, Work::First)),
            2
        );
        // a new node starts fresh
        assert_eq!(
            micros(cell(Segment::OffMain, TokenKind::CacheWrite, Work::First)),
            1
        );
        assert_eq!(split.objective().cost.micros(), 5);
    }

    #[test]
    fn sets_never_count_a_cell_twice() {
        let both = TermSet::of_kind(TokenKind::Output).union(&TermSet::of_work(Work::Rework));
        let run = Run::new(testkit::run_parts(
            Outcome::Success,
            vec![testkit::request(3, 5)],
        ))
        .unwrap();
        let split = CostBreakdown::of(&run).unwrap();
        assert_eq!(split.sum(&both), Observed::Known(Cost::from_micros(5)));
    }

    #[test]
    fn a_reported_cost_counts_even_without_parts() {
        let mut request = testkit::request(3, 5);
        request.cost.terms = Observed::Missing;
        request.cost.reported = Observed::Known(Cost::from_micros(9));
        let run = Run::new(testkit::run_parts(Outcome::Success, vec![request.clone()])).unwrap();
        let split = CostBreakdown::of(&run).unwrap();
        assert!(!split.has_terms());
        assert_eq!(split.objective().cost.micros(), 9);
        assert_eq!(split.objective().origin, CostOrigin::Reported);
        request.cost.reported = Observed::Missing;
        let run = Run::new(testkit::run_parts(Outcome::Success, vec![request])).unwrap();
        assert_eq!(CostBreakdown::of(&run), Err(CostError::Unknown));
    }

    use crate::domain::run::Outcome;
}
