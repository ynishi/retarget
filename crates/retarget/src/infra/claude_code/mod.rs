//! Claude Code's telemetry, translated into runs.
//!
//! Claude Code sends OpenTelemetry events when `CLAUDE_CODE_ENABLE_TELEMETRY=1`
//! and a logs exporter is set (`OTEL_LOGS_EXPORTER=otlp`, with the
//! `http/json` protocol for this reader). The events, not the metrics, are
//! read: only they carry each request's cost, its split by cache lifetime and
//! the agent that made it.
//!
//! ```text
//! payloads (OTLP/JSON)
//!   │ otlp: decode            the encoding → flat records
//!   ▼
//! records ── event: classify   Claude Code's names → the domain's values
//!   │                          (tokens, reported cost, tool calls, incidents)
//!   ▼
//! assemble                     sessions, their order, the tree of agents,
//!   │                          versions and duration; terms priced
//!   ▼
//! SessionTelemetry ── into_run(RunContext) ──▶ Run
//! ```
//!
//! # How a record is placed
//!
//! - A session is the records of one `session.id`; a record without one is
//!   left out and noted.
//! - A session's records are ordered by `event.sequence` when every record has
//!   it, else by their time when every record has one, else as they came.
//! - A record with an `agent_id` belongs to that subagent, a child of the
//!   first `parent_agent_id` its records name (of the main conversation when
//!   none does). Its children follow the session's order. An id labelled
//!   as compaction is auxiliary work. Without an `agent_id` (Claude Code before
//!   2.1.310), a record of the main thread belongs to the main conversation, a
//!   compaction's to an auxiliary node, one naming `agent.name` to one node
//!   per subagent type, and any other source to an auxiliary node of its own.
//! - A parent never seen in the session, or one that would close a loop, makes
//!   the subagent a child of the main conversation, and is noted.
//! - A subagent's role is its `agent.name`, else the `agent_type` its end
//!   named, else `unnamed`, lower-cased. A name that is not a role key becomes
//!   `custom`, and the original is noted.
//!
//! # What becomes an element
//!
//! - `api_request` becomes a request: its tokens, the cost reported (in micro
//!   dollars when sent, else dollars), and its terms when the price table has
//!   its model. The split by cache lifetime is kept only when it adds up to
//!   the total: Claude Code sends both parts as zero when the model's API did
//!   not report them.
//! - `tool_result` becomes a tool call when its tool has a kind here and its
//!   success could be read. Others are noted and left out, never counted as
//!   failures.
//! - `api_error`, `api_refusal` and `compaction` become incidents. An error is
//!   one incident whatever its attempt: what the attempt count restarts on is
//!   not settled, and a retry is not guessed.
//! - Every other event is left out.
//!
//! A session's duration is from its first record to its last, the left-out
//! ones included. Its harness version at each end is the `app.version` of the
//! first and last record that has one (sent only with
//! `OTEL_METRICS_INCLUDE_VERSION=true`); the caller supplies what it read at
//! each end otherwise.
//!
//! # Known limits
//!
//! - The reading follows Claude Code's published description of its events,
//!   checked against hand-written records, not yet against a captured
//!   session.
//! - Nothing links an Agent tool call to the subagent it started; the tree is
//!   built from the agents' parents.
//! - A subagent its user named `custom` cannot be told from the names that
//!   became `custom`.
//!
//! The caller decides when a session is complete: an exporter sends one
//! session in many payloads, so every payload of a session is passed in one
//! [`translate`].

mod assemble;
mod event;

use retarget_core::domain::name::{HarnessVersion, SessionId};
use retarget_core::domain::run::{
    AgentNode, Effective, Observed, Outcome, Run, RunError, RunParts, Snapshot, TaskClass,
};

use crate::infra::otlp::{self, OtlpError};
use crate::infra::price::{PriceGap, PriceTable};

/// The sessions found in a set of payloads, and what could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    /// The sessions, in the order each first appears.
    pub sessions: Vec<SessionTelemetry>,
    /// What was left out or replaced, and why.
    pub notes: Vec<Note>,
}

/// One session as its telemetry showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTelemetry {
    /// The harness's session id.
    pub session: SessionId,
    /// The harness version its first record carried.
    pub start_harness: Observed<HarnessVersion>,
    /// The harness version its last record carried.
    pub end_harness: Observed<HarnessVersion>,
    /// From its first record to its last.
    pub duration_ms: Observed<u64>,
    /// The main conversation and the agents below it.
    pub root: AgentNode,
}

/// What a run needs that the telemetry does not carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContext {
    /// The task class.
    pub task_class: TaskClass,
    /// Whether the task succeeded.
    pub outcome: Outcome,
    /// The positions in effect when it started.
    pub start: Effective,
    /// The positions in effect when it ended.
    pub end: Effective,
    /// The harness version read when it started, used when the telemetry has
    /// none.
    pub start_harness: Observed<HarnessVersion>,
    /// The harness version read when it ended, used when the telemetry has
    /// none.
    pub end_harness: Observed<HarnessVersion>,
}

/// Where a run's harness version came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessSource {
    /// The session's telemetry.
    Telemetry,
    /// The caller's context.
    Context,
}

/// A run made from a session's telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryRun {
    /// The run.
    pub run: Run,
    /// Where its starting harness version came from.
    pub start_source: HarnessSource,
    /// Where its ending harness version came from.
    pub end_source: HarnessSource,
}

/// Why a session's telemetry did not become a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IntoRunError {
    /// Neither the telemetry nor the context knows the harness version at an
    /// end of the session.
    #[error("the harness version at an end of the session is not known")]
    HarnessUnknown,
    /// The tree broke a rule of runs; this adapter has the bug.
    #[error("the session's tree is not a run")]
    Shape(#[source] RunError),
}

/// Something in the input that was left out or replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The session, when the record had one.
    pub session: Option<SessionId>,
    /// The record's position among all the records passed in.
    pub record: usize,
    /// What happened.
    pub kind: NoteKind,
}

/// What was left out or replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteKind {
    /// The record has no session id; it was left out.
    NoSession,
    /// A subagent's parent was not seen, or would close a loop; the subagent
    /// was placed under the main conversation.
    Orphan {
        /// The parent's id as sent.
        agent: String,
    },
    /// An attribute was there but could not be read; it was taken as missing.
    Unreadable(&'static str),
    /// A tool with no kind here; its call was left out.
    UnknownTool(String),
    /// A source of requests not known here; it became auxiliary work.
    UnknownSource(String),
    /// A subagent's name is not a role key; the role became `custom`.
    RoleReplaced(String),
    /// A request's terms could not be priced.
    Unpriced(PriceGap),
}

/// The sessions in a set of payloads. Every payload of a session must be
/// among them: a session split across two calls becomes two partial ones.
pub fn translate<'a>(
    payloads: impl IntoIterator<Item = &'a str>,
    prices: &PriceTable,
) -> Result<Translation, OtlpError> {
    let mut records = Vec::new();
    for payload in payloads {
        records.extend(otlp::decode(payload)?);
    }
    Ok(assemble::sessions(records, prices))
}

impl SessionTelemetry {
    /// The run of this session. Each end's harness version is the
    /// telemetry's, else the context's; with neither, there is no run.
    pub fn into_run(self, context: RunContext) -> Result<TelemetryRun, IntoRunError> {
        let pick = |telemetry: Observed<HarnessVersion>, given: Observed<HarnessVersion>| match (
            telemetry, given,
        ) {
            (Observed::Known(v), _) => Ok((v, HarnessSource::Telemetry)),
            (Observed::Missing, Observed::Known(v)) => Ok((v, HarnessSource::Context)),
            (Observed::Missing, Observed::Missing) => Err(IntoRunError::HarnessUnknown),
        };
        let (start_harness, start_source) = pick(self.start_harness, context.start_harness)?;
        let (end_harness, end_source) = pick(self.end_harness, context.end_harness)?;
        let run = Run::new(RunParts {
            session: self.session,
            task_class: context.task_class,
            outcome: context.outcome,
            duration_ms: self.duration_ms,
            start: Snapshot {
                harness: start_harness,
                effective: context.start,
            },
            end: Snapshot {
                harness: end_harness,
                effective: context.end,
            },
            root: self.root,
        })
        .map_err(IntoRunError::Shape)?;
        Ok(TelemetryRun {
            run,
            start_source,
            end_source,
        })
    }
}
