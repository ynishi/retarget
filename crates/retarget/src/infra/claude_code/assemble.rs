//! The records of every payload, grouped into sessions and built into each
//! session's tree of agents.

use std::collections::{BTreeMap, BTreeSet};

use retarget_core::domain::name::{HarnessVersion, SessionId};
use retarget_core::domain::run::{AgentNode, Element, Observed, Request, RequestCost, Role};

use super::event::{AgentRef, Event, NodeKey, NodeKind, Parent, RoleEvidence, role_of};
use super::{Note, NoteKind, SessionTelemetry, Translation};
use crate::infra::otlp::{LogRecord, Lookup};
use crate::infra::price::PriceTable;

/// One input record and where it came in.
struct Indexed {
    index: usize,
    record: LogRecord,
}

/// The sessions of the records, in the order each first appears.
pub(crate) fn sessions(records: Vec<LogRecord>, prices: &PriceTable) -> Translation {
    let mut notes = Vec::new();
    let mut order: Vec<SessionId> = Vec::new();
    let mut by_session: BTreeMap<SessionId, Vec<Indexed>> = BTreeMap::new();
    for (index, record) in records.into_iter().enumerate() {
        let session = match record.str("session.id") {
            Lookup::Value(s) => s
                .parse::<SessionId>()
                .map_err(|_| NoteKind::Unreadable("session.id")),
            Lookup::Absent => Err(NoteKind::NoSession),
            Lookup::Unreadable => Err(NoteKind::Unreadable("session.id")),
        };
        let session = match session {
            Ok(session) => session,
            Err(kind) => {
                notes.push(Note {
                    session: None,
                    record: index,
                    kind,
                });
                continue;
            }
        };
        let list = by_session.entry(session.clone()).or_default();
        if list.is_empty() {
            order.push(session);
        }
        list.push(Indexed { index, record });
    }
    let sessions = order
        .into_iter()
        .filter_map(|session| {
            let records = by_session.remove(&session)?;
            Some(one_session(session, records, prices, &mut notes))
        })
        .collect();
    Translation { sessions, notes }
}

/// Orders a session's records by one key for all of them: the harness's
/// sequence when every record has it, else the time when every record has
/// it, else the order they came in. Returns what could not be read, by
/// input index: a sequence or a time that was there but unreadable.
fn order(records: &mut [Indexed]) -> Vec<(usize, NoteKind)> {
    let mut unreadable = Vec::new();
    for r in records.iter() {
        if r.record.uint("event.sequence") == Lookup::Unreadable {
            unreadable.push((r.index, NoteKind::Unreadable("event.sequence")));
        }
        if r.record.time_nanos == Lookup::Unreadable {
            unreadable.push((r.index, NoteKind::Unreadable("timeUnixNano")));
        }
    }
    if records
        .iter()
        .all(|r| r.record.uint("event.sequence").value().is_some())
    {
        records.sort_by_cached_key(|r| (r.record.uint("event.sequence").value(), r.index));
    } else if records
        .iter()
        .all(|r| r.record.time_nanos.value().is_some())
    {
        records.sort_by_key(|r| (r.record.time_nanos.value(), r.index));
    } else {
        records.sort_by_key(|r| r.index);
    }
    unreadable
}

struct NodeBuild {
    kind: NodeKind,
    parent: Parent,
    evidence: RoleEvidence,
    elements: Vec<Element>,
    /// The input index of its first record, for notes.
    first: usize,
    /// Where its first record falls in the session's order; nodes and
    /// children are ordered by it, so the tree does not depend on the order
    /// the payloads came in.
    pos: usize,
}

fn one_session(
    session: SessionId,
    mut records: Vec<Indexed>,
    prices: &PriceTable,
    notes: &mut Vec<Note>,
) -> SessionTelemetry {
    let unreadable = order(&mut records);
    let mut note = |index: usize, kinds: Vec<NoteKind>| {
        notes.extend(kinds.into_iter().map(|kind| Note {
            session: Some(session.clone()),
            record: index,
            kind,
        }));
    };
    for (index, kind) in unreadable {
        note(index, vec![kind]);
    }

    let duration_ms = {
        let times: Option<Vec<u64>> = records
            .iter()
            .map(|r| r.record.time_nanos.value())
            .collect();
        let first = times.as_ref().and_then(|t| t.iter().min().copied());
        let last = times.as_ref().and_then(|t| t.iter().max().copied());
        match (first, last) {
            (Some(first), Some(last)) => Observed::Known((last - first) / 1_000_000),
            _ => Observed::Missing,
        }
    };
    let first_version = records.iter().position(|r| r.record.has("app.version"));
    let last_version = records.iter().rposition(|r| r.record.has("app.version"));
    // A version that is there but is not a string, or does not parse, is
    // unreadable; it is noted once even when one record is both ends.
    let mut harness_at = |at: Option<usize>, noted: bool| -> Observed<HarnessVersion> {
        let Some(r) = at.map(|i| &records[i]) else {
            return Observed::Missing;
        };
        match r.record.str("app.version").value().map(str::parse) {
            Some(Ok(v)) => Observed::Known(v),
            Some(Err(_)) | None => {
                if !noted {
                    note(r.index, vec![NoteKind::Unreadable("app.version")]);
                }
                Observed::Missing
            }
        }
    };
    let start_harness = harness_at(first_version, false);
    let end_harness = harness_at(last_version, last_version == first_version);

    let mut nodes: BTreeMap<NodeKey, NodeBuild> = BTreeMap::new();
    nodes.insert(
        NodeKey::Main,
        NodeBuild {
            kind: NodeKind::Main,
            parent: Parent::Main,
            evidence: RoleEvidence::default(),
            elements: Vec::new(),
            first: 0,
            pos: 0,
        },
    );
    for (pos, r) in records.iter().enumerate() {
        let mut found = Vec::new();
        let event = Event::classify(&r.record, &mut found);
        if event == Event::Ignored {
            note(r.index, found);
            continue;
        }
        let agent = AgentRef::of(&r.record, &mut found);
        let AgentRef {
            key,
            kind,
            parent,
            agent_name,
        } = agent;
        let node = nodes.entry(key.clone()).or_insert_with(|| NodeBuild {
            kind,
            parent: Parent::Main,
            evidence: RoleEvidence::default(),
            elements: Vec::new(),
            first: r.index,
            pos,
        });
        // A subagent's parent is the first one its records name, whichever
        // of them names it.
        if node.parent == Parent::Main {
            node.parent = parent;
        }
        if node.evidence.agent_name.is_none() {
            node.evidence.agent_name = match key {
                NodeKey::ByName(name) => Some(name),
                _ => agent_name,
            };
        }
        match event {
            Event::Request {
                tokens,
                reported,
                model,
            } => {
                let terms = match prices.terms(&model, &tokens) {
                    Ok(terms) => Observed::Known(terms),
                    Err(gap) => {
                        found.push(NoteKind::Unpriced(gap));
                        Observed::Missing
                    }
                };
                node.elements.push(Element::Request(Request {
                    tokens,
                    cost: RequestCost { reported, terms },
                    model,
                }));
            }
            Event::Tool(call) => node.elements.push(Element::ToolCall(call)),
            Event::Incident(incident) => node.elements.push(Element::Incident(incident)),
            Event::SubagentCompleted { agent_type } => {
                if node.evidence.agent_type.is_none() {
                    node.evidence.agent_type = agent_type;
                }
            }
            Event::Ignored => {}
        }
        note(r.index, found);
    }

    let children = children(&nodes, &mut note);
    let mut role_notes = Vec::new();
    let root = build(&NodeKey::Main, &mut nodes, &children, &mut role_notes);
    for (index, kind) in role_notes {
        note(index, vec![kind]);
    }
    SessionTelemetry {
        session,
        start_harness,
        end_harness,
        duration_ms,
        root,
    }
}

/// The children of every node, each list in the session's order. A parent
/// never seen, or one that would close a loop, makes the node a child of the
/// main conversation.
fn children(
    nodes: &BTreeMap<NodeKey, NodeBuild>,
    note: &mut impl FnMut(usize, Vec<NoteKind>),
) -> BTreeMap<NodeKey, Vec<NodeKey>> {
    let mut by_pos: Vec<(&NodeKey, &NodeBuild)> =
        nodes.iter().filter(|(k, _)| **k != NodeKey::Main).collect();
    by_pos.sort_by_key(|(_, n)| n.pos);
    let mut parents: BTreeMap<NodeKey, NodeKey> = BTreeMap::new();
    let mut children: BTreeMap<NodeKey, Vec<NodeKey>> = BTreeMap::new();
    for (key, node) in by_pos {
        let parent = match &node.parent {
            Parent::Main => NodeKey::Main,
            Parent::Agent(id) => {
                let wanted = NodeKey::Instance(id.clone());
                if nodes.contains_key(&wanted) && !closes_loop(key, &wanted, &parents) {
                    wanted
                } else {
                    note(node.first, vec![NoteKind::Orphan { agent: id.clone() }]);
                    NodeKey::Main
                }
            }
        };
        children
            .entry(parent.clone())
            .or_default()
            .push(key.clone());
        parents.insert(key.clone(), parent);
    }
    children
}

/// Whether making `parent` the parent of `child` would close a loop, given
/// the parents decided so far.
fn closes_loop(child: &NodeKey, parent: &NodeKey, parents: &BTreeMap<NodeKey, NodeKey>) -> bool {
    let mut seen = BTreeSet::new();
    let mut at = parent;
    loop {
        if at == child {
            return true;
        }
        if *at == NodeKey::Main || !seen.insert(at) {
            return false;
        }
        match parents.get(at) {
            Some(next) => at = next,
            None => return false,
        }
    }
}

fn build(
    key: &NodeKey,
    nodes: &mut BTreeMap<NodeKey, NodeBuild>,
    children: &BTreeMap<NodeKey, Vec<NodeKey>>,
    notes: &mut Vec<(usize, NoteKind)>,
) -> AgentNode {
    let Some(node) = nodes.remove(key) else {
        return AgentNode::new(Role::Main, vec![], vec![]);
    };
    let built = children
        .get(key)
        .into_iter()
        .flatten()
        .map(|child| build(child, nodes, children, notes))
        .collect();
    let role = match node.kind {
        NodeKind::Main => Role::Main,
        NodeKind::Aux(kind) => Role::Auxiliary(kind),
        NodeKind::Sub => {
            let mut found = Vec::new();
            let role = role_of(&node.evidence, &mut found);
            notes.extend(found.into_iter().map(|k| (node.first, k)));
            Role::Sub(role)
        }
    };
    AgentNode::new(role, node.elements, built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::otlp::AttrValue;

    fn rec(seq: Option<i64>, time: Option<u64>) -> LogRecord {
        let mut attrs = BTreeMap::new();
        if let Some(s) = seq {
            attrs.insert("event.sequence".to_owned(), AttrValue::Int(s));
        }
        LogRecord {
            attrs,
            body: None,
            time_nanos: time.map_or(Lookup::Absent, Lookup::Value),
        }
    }

    fn indexes(records: &[Indexed]) -> Vec<usize> {
        records.iter().map(|r| r.index).collect()
    }

    #[test]
    fn one_key_orders_every_record_of_a_session() {
        let mut all_seq: Vec<Indexed> = [(Some(2), Some(1)), (Some(1), Some(9)), (Some(1), None)]
            .into_iter()
            .enumerate()
            .map(|(index, (s, t))| Indexed {
                index,
                record: rec(s, t),
            })
            .collect();
        order(&mut all_seq);
        assert_eq!(indexes(&all_seq), vec![1, 2, 0]);

        let mut mixed: Vec<Indexed> = [(Some(5), Some(30)), (None, Some(10)), (Some(1), Some(20))]
            .into_iter()
            .enumerate()
            .map(|(index, (s, t))| Indexed {
                index,
                record: rec(s, t),
            })
            .collect();
        order(&mut mixed);
        assert_eq!(indexes(&mixed), vec![1, 2, 0]);

        let mut neither: Vec<Indexed> = [(Some(5), None), (None, Some(10))]
            .into_iter()
            .enumerate()
            .map(|(index, (s, t))| Indexed {
                index,
                record: rec(s, t),
            })
            .collect();
        order(&mut neither);
        assert_eq!(indexes(&neither), vec![0, 1]);
    }

    #[test]
    fn an_unreadable_session_sequence_or_time_is_noted_not_dropped_silently() {
        let with = |attrs: &[(&str, AttrValue)], time: Lookup<u64>| LogRecord {
            attrs: attrs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
            body: None,
            time_nanos: time,
        };
        let sid = AttrValue::Str("test-s".to_owned());
        let records = vec![
            with(&[], Lookup::Absent),
            with(&[("session.id", AttrValue::Int(7))], Lookup::Absent),
            with(
                &[
                    ("session.id", sid.clone()),
                    ("event.sequence", AttrValue::Str("x".to_owned())),
                ],
                Lookup::Unreadable,
            ),
        ];
        let t = sessions(records, &PriceTable::default());
        let kinds: Vec<(usize, &NoteKind)> = t.notes.iter().map(|n| (n.record, &n.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                (0, &NoteKind::NoSession),
                (1, &NoteKind::Unreadable("session.id")),
                (2, &NoteKind::Unreadable("event.sequence")),
                (2, &NoteKind::Unreadable("timeUnixNano")),
            ]
        );
        assert_eq!(t.sessions[0].duration_ms, Observed::Missing);
    }
}
