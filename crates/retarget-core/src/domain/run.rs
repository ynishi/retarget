//! One session of the agent, observed as a run: a tree of agent nodes and what
//! happened in each.
//!
//! The kinds of element are few and closed; the configuration of agents is not,
//! so it is a value, one tree per run. A value the harness did not send is
//! [`Observed::Missing`], never guessed.
//!
//! ```text
//! Run ── start / end Snapshot (harness version, effective position per lever)
//!  └─ AgentNode (Main)
//!      ├─ Element::Request | ToolCall | Incident   (in the order they happened)
//!      └─ AgentNode (Sub(role) | Auxiliary(kind)) ...
//! ```
//!
//! # Invariants
//!
//! - The root node is the main conversation, and no other node is.
//! - Whether a run spans a change is decided by [`Run::span`] alone.
//!
//! # Errors
//!
//! [`RunError`] says which shape rule the tree broke; the adapter that built
//! the tree has the bug.

use std::collections::BTreeMap;

use crate::domain::money::{Cost, TermCosts};
use crate::domain::name::{
    HarnessVersion, Language, LeverKey, ModelId, PositionId, ProjectId, RoleKey, SessionId,
};

/// A value the harness may or may not have sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed<T> {
    /// The harness sent it.
    Known(T),
    /// The harness did not send it; nothing is guessed in its place.
    Missing,
}

impl<T> Observed<T> {
    /// The value, when it is known.
    pub fn known(&self) -> Option<&T> {
        match self {
            Observed::Known(value) => Some(value),
            Observed::Missing => None,
        }
    }
}

/// What part of the agent a node is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// The main conversation.
    Main,
    /// A subagent of a role.
    Sub(RoleKey),
    /// Work the harness does on its own behalf.
    Auxiliary(AuxKind),
}

/// The kinds of auxiliary work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxKind {
    /// Summarising the conversation to free context.
    Compaction,
    /// Other background work.
    Background,
    /// Anything else the harness reports.
    Other,
}

/// The tokens of one request, as the harness counted them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenUse {
    /// Input neither read from nor written to the cache.
    pub uncached_input: Observed<u64>,
    /// Output.
    pub output: Observed<u64>,
    /// Input read from the cache.
    pub cache_read: Observed<u64>,
    /// Input written to the five-minute cache.
    pub cache_write_5m: Observed<u64>,
    /// Input written to the one-hour cache.
    pub cache_write_1h: Observed<u64>,
}

/// What a request cost: the total the harness reported, and the parts an
/// adapter priced from the tokens. Either may be missing; neither is guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestCost {
    /// The cost the harness reported, used as it is.
    pub reported: Observed<Cost>,
    /// The cost split by kind, priced from the tokens and a table of prices
    /// (both cache lifetimes counted as writes).
    pub terms: Observed<TermCosts>,
}

/// One request to a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Its tokens.
    pub tokens: TokenUse,
    /// Its cost.
    pub cost: RequestCost,
    /// The model it went to.
    pub model: Observed<ModelId>,
}

/// The kinds of tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    /// Reading files.
    Read,
    /// Editing files.
    Edit,
    /// Running commands.
    Exec,
    /// Searching.
    Search,
    /// Starting a subagent.
    Agent,
    /// Anything outside the machine.
    External,
}

/// How a tool call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolResult {
    /// It worked.
    Ok,
    /// It failed.
    Err,
}

/// One tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolCall {
    /// The kind of tool.
    pub class: ToolClass,
    /// How it ended.
    pub result: ToolResult,
}

/// Something that happened and costs work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incident {
    /// A request was retried.
    Retry,
    /// The model's API returned an error.
    ApiError,
    /// The model refused.
    Refusal,
    /// The conversation was compacted.
    Compaction,
    /// The task was abandoned.
    Abandon,
}

/// One thing that happened in a node, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Element {
    /// A model request.
    Request(Request),
    /// A tool call.
    ToolCall(ToolCall),
    /// An incident.
    Incident(Incident),
}

/// One part of the agent and what happened in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentNode {
    role: Role,
    elements: Vec<Element>,
    children: Vec<AgentNode>,
}

impl AgentNode {
    /// A node. Its shape is checked when it becomes part of a [`Run`].
    pub fn new(role: Role, elements: Vec<Element>, children: Vec<AgentNode>) -> Self {
        Self {
            role,
            elements,
            children,
        }
    }
    /// What part of the agent it is.
    pub fn role(&self) -> &Role {
        &self.role
    }
    /// What happened in it, in order.
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }
    /// The nodes it started.
    pub fn children(&self) -> &[AgentNode] {
        &self.children
    }
    fn any(&self, test: &dyn Fn(&AgentNode) -> bool) -> bool {
        test(self) || self.children.iter().any(|child| child.any(test))
    }
}

/// The position each lever actually had, as the run observed it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Effective(BTreeMap<LeverKey, Observed<PositionId>>);

impl Effective {
    /// The observed positions.
    pub fn new(positions: BTreeMap<LeverKey, Observed<PositionId>>) -> Self {
        Self(positions)
    }
    /// What was observed of one lever, when it was looked at.
    pub fn get(&self, lever: &LeverKey) -> Option<&Observed<PositionId>> {
        self.0.get(lever)
    }
}

/// The configuration at one end of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The harness's version.
    pub harness: HarnessVersion,
    /// The positions that were in effect.
    pub effective: Effective,
}

/// Whether a run kept one configuration from start to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// It did.
    Within,
    /// It did not: it counts for neither arm.
    Spans,
}

/// The size of a task, as a band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SizeBand {
    /// Small.
    Small,
    /// Medium.
    Medium,
    /// Large.
    Large,
}

/// The group a run is compared within, so that unlike tasks are never compared.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskClass {
    /// The project.
    pub project: ProjectId,
    /// The main language.
    pub language: Language,
    /// The size band.
    pub size: SizeBand,
}

/// Whether the task succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It did.
    Success,
    /// It did not.
    Failure,
    /// Nobody can tell; the run is left out of every comparison.
    Unknown,
}

/// Why a tree is not a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    /// The root is not the main conversation.
    #[error("the root of a run must be the main conversation")]
    RootNotMain,
    /// A node below the root claims to be the main conversation.
    #[error("only the root of a run may be the main conversation")]
    NestedMain,
}

/// One session of the agent, taken as one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    session: SessionId,
    task_class: TaskClass,
    outcome: Outcome,
    duration_ms: Observed<u64>,
    start: Snapshot,
    end: Snapshot,
    root: AgentNode,
}

/// The parts of a run, given to [`Run::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunParts {
    /// The harness's session id.
    pub session: SessionId,
    /// The task class.
    pub task_class: TaskClass,
    /// Whether the task succeeded.
    pub outcome: Outcome,
    /// How long it took.
    pub duration_ms: Observed<u64>,
    /// The configuration when it started.
    pub start: Snapshot,
    /// The configuration when it ended.
    pub end: Snapshot,
    /// The main conversation and everything below it.
    pub root: AgentNode,
}

impl Run {
    /// A run, refused when its tree has the wrong shape.
    pub fn new(parts: RunParts) -> Result<Self, RunError> {
        if parts.root.role != Role::Main {
            return Err(RunError::RootNotMain);
        }
        if parts
            .root
            .children
            .iter()
            .any(|child| child.any(&|node| node.role == Role::Main))
        {
            return Err(RunError::NestedMain);
        }
        Ok(Self {
            session: parts.session,
            task_class: parts.task_class,
            outcome: parts.outcome,
            duration_ms: parts.duration_ms,
            start: parts.start,
            end: parts.end,
            root: parts.root,
        })
    }
    /// The harness's session id.
    pub fn session(&self) -> &SessionId {
        &self.session
    }
    /// The task class.
    pub fn task_class(&self) -> &TaskClass {
        &self.task_class
    }
    /// Whether the task succeeded.
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
    /// How long it took, in milliseconds.
    pub fn duration_ms(&self) -> Observed<u64> {
        self.duration_ms
    }
    /// The configuration when it started.
    pub fn start(&self) -> &Snapshot {
        &self.start
    }
    /// The configuration when it ended.
    pub fn end(&self) -> &Snapshot {
        &self.end
    }
    /// The main conversation.
    pub fn root(&self) -> &AgentNode {
        &self.root
    }
    /// Whether some node of the run has a role.
    pub fn has_role(&self, role: &RoleKey) -> bool {
        self.root
            .any(&|node| matches!(&node.role, Role::Sub(key) if key == role))
    }
    /// Whether the run kept one configuration. A different harness version
    /// spans; so does a lever whose position is known at both ends and differs.
    /// A position missing at either end decides nothing.
    pub fn span(&self) -> Span {
        if self.start.harness != self.end.harness {
            return Span::Spans;
        }
        let changed = self.start.effective.0.iter().any(|(lever, start)| {
            match (start, self.end.effective.get(lever)) {
                (Observed::Known(a), Some(Observed::Known(b))) => a != b,
                _ => false,
            }
        });
        if changed { Span::Spans } else { Span::Within }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::testkit;

    #[test]
    fn the_root_is_the_only_main() {
        let mut parts = testkit::run_parts(Outcome::Success, vec![]);
        parts.root = AgentNode::new(Role::Auxiliary(AuxKind::Other), vec![], vec![]);
        assert_eq!(Run::new(parts.clone()), Err(RunError::RootNotMain));
        parts.root = AgentNode::new(
            Role::Main,
            vec![],
            vec![AgentNode::new(
                Role::Sub("r".parse().unwrap()),
                vec![],
                vec![AgentNode::new(Role::Main, vec![], vec![])],
            )],
        );
        assert_eq!(Run::new(parts), Err(RunError::NestedMain));
    }

    #[test]
    fn span_looks_at_versions_and_known_positions_only() {
        let lever: LeverKey = "l".parse().unwrap();
        let at = |p: Observed<PositionId>| Effective::new([(lever.clone(), p)].into());
        let a = Observed::Known("a".parse().unwrap());
        let b = Observed::Known("b".parse().unwrap());
        let cases = [
            ("2.0.0", at(a.clone()), "2.0.0", at(a.clone()), Span::Within),
            ("2.0.0", at(a.clone()), "2.0.1", at(a.clone()), Span::Spans),
            ("2.0.0", at(a.clone()), "2.0.0", at(b), Span::Spans),
            ("2.0.0", at(a), "2.0.0", at(Observed::Missing), Span::Within),
        ];
        for (start_v, start_e, end_v, end_e, want) in cases {
            let mut parts = testkit::run_parts(Outcome::Success, vec![]);
            parts.start = Snapshot {
                harness: start_v.parse().unwrap(),
                effective: start_e,
            };
            parts.end = Snapshot {
                harness: end_v.parse().unwrap(),
                effective: end_e,
            };
            assert_eq!(Run::new(parts).unwrap().span(), want);
        }
    }
}
