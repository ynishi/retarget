//! Claude Code's vocabulary: which event a record is, which agent it belongs
//! to, and what it says in the domain's terms. Every event and attribute name
//! of the harness is here and nowhere else.

use retarget_core::domain::money::Cost;
use retarget_core::domain::name::{ModelId, RoleKey};
use retarget_core::domain::run::{
    AuxKind, Incident, Observed, TokenUse, ToolCall, ToolClass, ToolResult,
};

use super::NoteKind;
use crate::infra::otlp::{LogRecord, Lookup};

const PREFIX: &str = "claude_code.";
/// The `agent_id` of a compaction starts with this label.
const COMPACTION_AGENT: &str = "acompact";
const COMPACTION_SOURCE: &str = "compact";
const MAIN_SOURCE: &str = "repl_main_thread";

/// What one record says, in the domain's terms.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    /// A model request, not yet priced by kind.
    Request {
        tokens: TokenUse,
        reported: Observed<Cost>,
        model: Observed<ModelId>,
    },
    /// A tool call whose kind and result were read.
    Tool(ToolCall),
    /// Something that costs work.
    Incident(Incident),
    /// A subagent ended; it names the subagent's type.
    SubagentCompleted { agent_type: Option<String> },
    /// An event that does not enter the tree (a prompt, a hook, a decision),
    /// or a tool call that could not be read (a note says why).
    Ignored,
}

impl Event {
    /// The event a record is. What could not be read is noted, never guessed.
    pub(crate) fn classify(record: &LogRecord, notes: &mut Vec<NoteKind>) -> Event {
        let Some(name) = name(record, notes) else {
            return Event::Ignored;
        };
        match name {
            "api_request" => request(record, notes),
            "api_error" => Event::Incident(Incident::ApiError),
            "api_refusal" => Event::Incident(Incident::Refusal),
            "compaction" => Event::Incident(Incident::Compaction),
            "tool_result" => tool(record, notes),
            "subagent_completed" => Event::SubagentCompleted {
                agent_type: text(record, "agent_type", notes).map(str::to_owned),
            },
            _ => Event::Ignored,
        }
    }
}

/// The event's name without the harness's prefix: from `event.name`, or the
/// body when that is absent. Both the bare and the prefixed form are taken.
fn name<'r>(record: &'r LogRecord, notes: &mut Vec<NoteKind>) -> Option<&'r str> {
    let raw = text(record, "event.name", notes).or(record.body.as_deref())?;
    Some(raw.strip_prefix(PREFIX).unwrap_or(raw))
}

fn request(record: &LogRecord, notes: &mut Vec<NoteKind>) -> Event {
    let mut count = |key: &'static str| match record.uint(key) {
        Lookup::Value(n) => Observed::Known(n),
        Lookup::Absent => Observed::Missing,
        Lookup::Unreadable => {
            notes.push(NoteKind::Unreadable(key));
            Observed::Missing
        }
    };
    let uncached_input = count("input_tokens");
    let output = count("output_tokens");
    let cache_read = count("cache_read_tokens");
    let total = count("cache_creation_tokens");
    let m5 = count("cache_creation_5m_tokens");
    let h1 = count("cache_creation_1h_tokens");
    let writes = cache_writes(total, m5, h1);
    if writes.inconsistent {
        notes.push(NoteKind::Unreadable("cache_creation_5m_tokens"));
    }
    let model = match text(record, "model", notes).map(str::parse::<ModelId>) {
        Some(Ok(m)) => Observed::Known(m),
        Some(Err(_)) => {
            notes.push(NoteKind::Unreadable("model"));
            Observed::Missing
        }
        None => Observed::Missing,
    };
    Event::Request {
        tokens: TokenUse {
            uncached_input,
            output,
            cache_read,
            cache_write_5m: writes.m5,
            cache_write_1h: writes.h1,
        },
        reported: reported_cost(record, notes),
        model,
    }
}

/// A string attribute. One that is there but is not a string is noted and
/// taken as absent.
fn text<'r>(
    record: &'r LogRecord,
    key: &'static str,
    notes: &mut Vec<NoteKind>,
) -> Option<&'r str> {
    match record.str(key) {
        Lookup::Value(s) => Some(s),
        Lookup::Absent => None,
        Lookup::Unreadable => {
            notes.push(NoteKind::Unreadable(key));
            None
        }
    }
}

/// The cost the harness reported: the integer micro dollars when sent, else
/// the dollars, rounded to a micro dollar.
fn reported_cost(record: &LogRecord, notes: &mut Vec<NoteKind>) -> Observed<Cost> {
    match record.uint("cost_usd_micros") {
        Lookup::Value(m) => return Observed::Known(Cost::from_micros(m)),
        Lookup::Unreadable => notes.push(NoteKind::Unreadable("cost_usd_micros")),
        Lookup::Absent => {}
    }
    match record.number("cost_usd") {
        Lookup::Value(usd) if usd >= 0.0 && usd * 1e6 < u64::MAX as f64 => {
            Observed::Known(Cost::from_micros((usd * 1e6).round() as u64))
        }
        Lookup::Absent => Observed::Missing,
        Lookup::Value(_) | Lookup::Unreadable => {
            notes.push(NoteKind::Unreadable("cost_usd"));
            Observed::Missing
        }
    }
}

/// The tokens written to each cache lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CacheWrites {
    pub(crate) m5: Observed<u64>,
    pub(crate) h1: Observed<u64>,
    /// The split was sent, is not zero, and does not add up to the total.
    pub(crate) inconsistent: bool,
}

/// How the cache writes split between the two lifetimes. The harness sends
/// both parts as zero when the API did not report them, even when the total
/// is not zero; then the split is not known, and neither is it when both are
/// zero and no total was sent.
pub(crate) fn cache_writes(
    total: Observed<u64>,
    m5: Observed<u64>,
    h1: Observed<u64>,
) -> CacheWrites {
    let split = |m5, h1, inconsistent| CacheWrites {
        m5,
        h1,
        inconsistent,
    };
    let nonzero = |o: Observed<u64>| matches!(o, Observed::Known(n) if n != 0);
    let any_nonzero = nonzero(m5) || nonzero(h1);
    match (total, m5, h1) {
        (Observed::Missing, Observed::Known(_), Observed::Known(_)) if any_nonzero => {
            split(m5, h1, false)
        }
        (Observed::Known(t), Observed::Known(a), Observed::Known(b))
            if any_nonzero && a.checked_add(b) == Some(t) =>
        {
            split(m5, h1, false)
        }
        (Observed::Known(0), _, _) if !any_nonzero => {
            split(Observed::Known(0), Observed::Known(0), false)
        }
        // A part that is not zero contradicts a total of zero, and two parts
        // contradict a total they do not add up to. One part alone and no
        // total decide nothing.
        (Observed::Known(t), _, _) => {
            let both = m5 != Observed::Missing && h1 != Observed::Missing;
            split(
                Observed::Missing,
                Observed::Missing,
                any_nonzero && (t == 0 || both),
            )
        }
        (Observed::Missing, _, _) => split(Observed::Missing, Observed::Missing, false),
    }
}

fn tool(record: &LogRecord, notes: &mut Vec<NoteKind>) -> Event {
    let Lookup::Value(tool_name) = record.str("tool_name") else {
        notes.push(NoteKind::Unreadable("tool_name"));
        return Event::Ignored;
    };
    let Some(class) = tool_class(tool_name) else {
        notes.push(NoteKind::UnknownTool(tool_name.to_owned()));
        return Event::Ignored;
    };
    let Lookup::Value(success) = record.bool("success") else {
        notes.push(NoteKind::Unreadable("success"));
        return Event::Ignored;
    };
    Event::Tool(ToolCall {
        class,
        result: if success {
            ToolResult::Ok
        } else {
            ToolResult::Err
        },
    })
}

/// The kind of a tool, for the harness's built-in tools and MCP servers'.
/// Another tool has no kind here.
pub(crate) fn tool_class(name: &str) -> Option<ToolClass> {
    if name.starts_with("mcp__") {
        return Some(ToolClass::External);
    }
    Some(match name {
        "Read" | "NotebookRead" => ToolClass::Read,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => ToolClass::Edit,
        "Bash" | "BashOutput" | "KillShell" | "KillBash" => ToolClass::Exec,
        "Grep" | "Glob" | "LS" | "ToolSearch" => ToolClass::Search,
        "WebSearch" | "WebFetch" => ToolClass::External,
        "Agent" | "Task" => ToolClass::Agent,
        _ => return None,
    })
}

/// Which node of the run's tree a record belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NodeKey {
    Main,
    /// A subagent or compaction with its own `agent_id`.
    Instance(String),
    /// Subagents of one type, from a harness that sends no `agent_id`.
    ByName(String),
    /// Compaction of the main conversation.
    MainCompaction,
    /// Other work the harness did on its own behalf, by its source.
    Source(String),
}

/// What kind of node a key is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Main,
    Sub,
    Aux(AuxKind),
}

/// Whose child a node is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Parent {
    Main,
    Agent(String),
}

/// The agent one record belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRef {
    pub(crate) key: NodeKey,
    pub(crate) kind: NodeKind,
    pub(crate) parent: Parent,
    /// The subagent's type, when the record names it.
    pub(crate) agent_name: Option<String>,
}

impl AgentRef {
    /// The agent of a record. With an `agent_id`, the record belongs to that
    /// subagent (or compaction). Without one: compaction, the main thread, a
    /// named subagent of an older harness, or other work, in that order.
    pub(crate) fn of(record: &LogRecord, notes: &mut Vec<NoteKind>) -> AgentRef {
        let agent_name = text(record, "agent.name", notes).map(str::to_owned);
        let source = text(record, "query_source", notes);
        if let Some(id) = text(record, "agent_id", notes) {
            let parent = match text(record, "parent_agent_id", notes) {
                Some(p) => Parent::Agent(p.to_owned()),
                None => Parent::Main,
            };
            let kind = if id.starts_with(COMPACTION_AGENT) || source == Some(COMPACTION_SOURCE) {
                NodeKind::Aux(AuxKind::Compaction)
            } else {
                NodeKind::Sub
            };
            return AgentRef {
                key: NodeKey::Instance(id.to_owned()),
                kind,
                parent,
                agent_name,
            };
        }
        let (key, kind) = match (source, &agent_name) {
            (Some(COMPACTION_SOURCE), _) => {
                (NodeKey::MainCompaction, NodeKind::Aux(AuxKind::Compaction))
            }
            (None, _) => (NodeKey::Main, NodeKind::Main),
            (Some(s), _) if s.starts_with(MAIN_SOURCE) => (NodeKey::Main, NodeKind::Main),
            (Some(_), Some(name)) => (NodeKey::ByName(name.clone()), NodeKind::Sub),
            (Some(s), None) => {
                notes.push(NoteKind::UnknownSource(s.to_owned()));
                (NodeKey::Source(s.to_owned()), NodeKind::Aux(AuxKind::Other))
            }
        };
        AgentRef {
            key,
            kind,
            parent: Parent::Main,
            agent_name,
        }
    }
}

/// What the records of one subagent said about its type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoleEvidence {
    pub(crate) agent_name: Option<String>,
    pub(crate) agent_type: Option<String>,
}

/// The role of a subagent: its `agent.name`, else the `agent_type` its end
/// named, else `unnamed`. A name that is not a role key once lower-cased
/// becomes `custom` (as the harness itself names agents it does not publish),
/// and the original is noted.
pub(crate) fn role_of(evidence: &RoleEvidence, notes: &mut Vec<NoteKind>) -> RoleKey {
    let fallback = |key: &str| key.parse::<RoleKey>();
    let Some(name) = evidence
        .agent_name
        .as_ref()
        .or(evidence.agent_type.as_ref())
    else {
        return fallback("unnamed").unwrap_or_else(|_| unreachable!("a valid key"));
    };
    match name.to_lowercase().parse() {
        Ok(key) => key,
        Err(_) => {
            notes.push(NoteKind::RoleReplaced(name.clone()));
            fallback("custom").unwrap_or_else(|_| unreachable!("a valid key"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::otlp::AttrValue;
    use std::collections::BTreeMap;

    /// Attributes, and the node they place a record in.
    type Placement<'a> = (&'a [(&'a str, AttrValue)], NodeKey, NodeKind, Parent);

    fn record(attrs: &[(&str, AttrValue)]) -> LogRecord {
        LogRecord {
            attrs: attrs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            body: None,
            time_nanos: Lookup::Absent,
        }
    }

    fn s(v: &str) -> AttrValue {
        AttrValue::Str(v.to_owned())
    }

    #[test]
    fn the_cache_split_is_kept_only_when_it_adds_up() {
        use Observed::{Known, Missing};
        let cases = [
            // total, 5m, 1h => 5m, 1h, inconsistent
            ((Known(10), Known(4), Known(6)), (Known(4), Known(6), false)),
            ((Missing, Known(4), Known(6)), (Known(4), Known(6), false)),
            ((Known(10), Known(0), Known(0)), (Missing, Missing, false)),
            ((Known(10), Known(4), Known(4)), (Missing, Missing, true)),
            ((Known(0), Known(0), Known(0)), (Known(0), Known(0), false)),
            ((Known(0), Missing, Missing), (Known(0), Known(0), false)),
            ((Known(10), Missing, Missing), (Missing, Missing, false)),
            ((Missing, Missing, Missing), (Missing, Missing, false)),
            // Both zero and no total: the harness's "not reported".
            ((Missing, Known(0), Known(0)), (Missing, Missing, false)),
            ((Known(0), Known(5), Missing), (Missing, Missing, true)),
            ((Known(5), Known(5), Missing), (Missing, Missing, false)),
        ];
        for ((total, m5, h1), (e5, e1, bad)) in cases {
            let w = cache_writes(total, m5, h1);
            assert_eq!(
                (w.m5, w.h1, w.inconsistent),
                (e5, e1, bad),
                "{total:?} {m5:?} {h1:?}"
            );
        }
    }

    #[test]
    fn tools_have_a_kind_only_when_known() {
        assert_eq!(tool_class("Read"), Some(ToolClass::Read));
        assert_eq!(tool_class("MultiEdit"), Some(ToolClass::Edit));
        assert_eq!(tool_class("Bash"), Some(ToolClass::Exec));
        assert_eq!(tool_class("Grep"), Some(ToolClass::Search));
        assert_eq!(tool_class("WebFetch"), Some(ToolClass::External));
        assert_eq!(tool_class("mcp__notes__get"), Some(ToolClass::External));
        assert_eq!(tool_class("Task"), Some(ToolClass::Agent));
        assert_eq!(tool_class("TodoWrite"), None);
    }

    #[test]
    fn a_tool_result_without_a_readable_success_makes_no_tool_call() {
        let mut notes = vec![];
        let ev = Event::classify(
            &record(&[("event.name", s("tool_result")), ("tool_name", s("Bash"))]),
            &mut notes,
        );
        assert_eq!(ev, Event::Ignored);
        assert_eq!(notes, vec![NoteKind::Unreadable("success")]);

        let mut notes = vec![];
        let ev = Event::classify(
            &record(&[
                ("event.name", s("claude_code.tool_result")),
                ("tool_name", s("Bash")),
                ("success", s("false")),
            ]),
            &mut notes,
        );
        assert_eq!(
            ev,
            Event::Tool(ToolCall {
                class: ToolClass::Exec,
                result: ToolResult::Err
            })
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn an_api_error_is_one_incident_whatever_its_attempt() {
        let mut notes = vec![];
        let ev = Event::classify(
            &record(&[
                ("event.name", s("api_error")),
                ("attempt", AttrValue::Int(2)),
            ]),
            &mut notes,
        );
        assert_eq!(ev, Event::Incident(Incident::ApiError));
    }

    #[test]
    fn the_reported_cost_prefers_micros_and_never_guesses() {
        let mut notes = vec![];
        let micros = record(&[
            ("cost_usd_micros", AttrValue::Int(1234)),
            ("cost_usd", AttrValue::Double(9.0)),
        ]);
        assert_eq!(
            reported_cost(&micros, &mut notes),
            Observed::Known(Cost::from_micros(1234))
        );
        let usd = record(&[("cost_usd", AttrValue::Double(0.0012345))]);
        assert_eq!(
            reported_cost(&usd, &mut notes),
            Observed::Known(Cost::from_micros(1235))
        );
        assert!(notes.is_empty());
        let bad = record(&[("cost_usd", AttrValue::Double(-1.0))]);
        assert_eq!(reported_cost(&bad, &mut notes), Observed::Missing);
        assert_eq!(notes, vec![NoteKind::Unreadable("cost_usd")]);
    }

    #[test]
    fn a_record_belongs_to_the_agent_its_attributes_name() {
        let cases: [Placement; 7] = [
            (&[], NodeKey::Main, NodeKind::Main, Parent::Main),
            (
                &[("query_source", s("repl_main_thread:outputStyle:x"))],
                NodeKey::Main,
                NodeKind::Main,
                Parent::Main,
            ),
            (
                &[("query_source", s("compact"))],
                NodeKey::MainCompaction,
                NodeKind::Aux(AuxKind::Compaction),
                Parent::Main,
            ),
            (
                &[
                    ("agent_id", s("test-a1")),
                    ("parent_agent_id", s("test-a0")),
                ],
                NodeKey::Instance("test-a1".into()),
                NodeKind::Sub,
                Parent::Agent("test-a0".into()),
            ),
            (
                &[("agent_id", s("acompact-test1"))],
                NodeKey::Instance("acompact-test1".into()),
                NodeKind::Aux(AuxKind::Compaction),
                Parent::Main,
            ),
            (
                &[("query_source", s("explore")), ("agent.name", s("Explore"))],
                NodeKey::ByName("Explore".into()),
                NodeKind::Sub,
                Parent::Main,
            ),
            (
                &[("query_source", s("title_generation"))],
                NodeKey::Source("title_generation".into()),
                NodeKind::Aux(AuxKind::Other),
                Parent::Main,
            ),
        ];
        for (attrs, key, kind, parent) in cases {
            let r = AgentRef::of(&record(attrs), &mut vec![]);
            assert_eq!((r.key, r.kind, r.parent), (key, kind, parent), "{attrs:?}");
        }
    }

    #[test]
    fn an_agent_attribute_that_is_not_a_string_is_noted() {
        let mut notes = vec![];
        let r = AgentRef::of(
            &record(&[
                ("agent_id", AttrValue::Int(7)),
                ("query_source", s("repl_main_thread")),
            ]),
            &mut notes,
        );
        assert_eq!(r.key, NodeKey::Main);
        assert_eq!(notes, vec![NoteKind::Unreadable("agent_id")]);

        let mut notes = vec![];
        let r = AgentRef::of(
            &record(&[
                ("agent_id", s("test-a1")),
                ("parent_agent_id", AttrValue::Bool(true)),
            ]),
            &mut notes,
        );
        assert_eq!(r.parent, Parent::Main);
        assert_eq!(notes, vec![NoteKind::Unreadable("parent_agent_id")]);
    }

    #[test]
    fn a_role_is_the_lower_cased_name_or_a_noted_stand_in() {
        let ev = |name: Option<&str>, ty: Option<&str>| RoleEvidence {
            agent_name: name.map(str::to_owned),
            agent_type: ty.map(str::to_owned),
        };
        let mut notes = vec![];
        assert_eq!(
            role_of(&ev(Some("Explore"), Some("x")), &mut notes).as_str(),
            "explore"
        );
        assert_eq!(
            role_of(&ev(None, Some("general-purpose")), &mut notes).as_str(),
            "general-purpose"
        );
        assert_eq!(role_of(&ev(None, None), &mut notes).as_str(), "unnamed");
        assert!(notes.is_empty());
        assert_eq!(
            role_of(&ev(Some("plugin:judge"), None), &mut notes).as_str(),
            "custom"
        );
        assert_eq!(notes, vec![NoteKind::RoleReplaced("plugin:judge".into())]);
    }
}
