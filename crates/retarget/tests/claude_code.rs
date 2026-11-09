//! Claude Code's telemetry, translated into runs, from hand-written records
//! that follow the harness's published description of its events.
//!
//! The fixtures are synthetic: every id starts with `test-` and no record
//! carries an account, user or organisation attribute (the last test checks
//! it), so none needs anonymising.

use std::collections::BTreeMap;
use std::path::Path;

use retarget::domain::money::{Cost, TokenKind};
use retarget::domain::name::{HarnessVersion, ModelId};
use retarget::domain::run::{
    AgentNode, AuxKind, Effective, Element, Incident, Observed, Outcome, Role, SizeBand, Span,
    TaskClass, ToolCall, ToolClass, ToolResult,
};
use retarget::infra::claude_code::{
    HarnessSource, IntoRunError, NoteKind, RunContext, SessionTelemetry, translate,
};
use retarget::infra::price::{ModelPrices, PriceGap, PriceTable};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude_code")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn prices() -> PriceTable {
    PriceTable::new(BTreeMap::from([(
        "test-model".parse::<ModelId>().unwrap(),
        ModelPrices {
            uncached_input: 3_000_000,
            output: 15_000_000,
            cache_read: 300_000,
            cache_write_5m: 3_750_000,
            cache_write_1h: 6_000_000,
        },
    )]))
}

fn context(harness: Observed<HarnessVersion>) -> RunContext {
    RunContext {
        task_class: TaskClass {
            project: "test-project".parse().unwrap(),
            language: "rust".parse().unwrap(),
            size: SizeBand::Small,
        },
        outcome: Outcome::Success,
        start: Effective::default(),
        end: Effective::default(),
        start_harness: harness,
        end_harness: harness,
    }
}

fn sub(name: &str) -> Role {
    Role::Sub(name.parse().unwrap())
}

/// Each element of a node as a short word, to compare a tree at a glance.
fn shape(node: &AgentNode) -> Vec<&'static str> {
    node.elements()
        .iter()
        .map(|e| match e {
            Element::Request(_) => "request",
            Element::ToolCall(ToolCall {
                class: ToolClass::Agent,
                result: ToolResult::Ok,
            }) => "agent ok",
            Element::ToolCall(ToolCall {
                class: ToolClass::Read,
                result: ToolResult::Ok,
            }) => "read ok",
            Element::ToolCall(ToolCall {
                class: ToolClass::Exec,
                result: ToolResult::Err,
            }) => "exec err",
            Element::ToolCall(_) => "other tool",
            Element::Incident(Incident::ApiError) => "api error",
            Element::Incident(Incident::Compaction) => "compaction",
            Element::Incident(_) => "other incident",
        })
        .collect()
}

fn swarm() -> SessionTelemetry {
    let (one, two) = (fixture("swarm-1.json"), fixture("swarm-2.json"));
    let mut t = translate([one.as_str(), two.as_str()], &prices()).unwrap();
    assert_eq!(t.sessions.len(), 1);
    t.sessions.remove(0)
}

#[test]
fn a_session_sent_in_two_payloads_becomes_one_tree_of_agents() {
    let s = swarm();
    let root = &s.root;
    assert_eq!(root.role(), &Role::Main);
    assert_eq!(shape(root), vec!["request", "agent ok", "compaction"]);

    let roles: Vec<&Role> = root.children().iter().map(AgentNode::role).collect();
    assert_eq!(
        roles,
        vec![
            &sub("explore"),
            &Role::Auxiliary(AuxKind::Compaction),
            &sub("unnamed"),
        ]
    );
    let explore = &root.children()[0];
    assert_eq!(shape(explore), vec!["request", "read ok"]);
    assert_eq!(explore.children().len(), 1);
    let nested = &explore.children()[0];
    assert_eq!(nested.role(), &sub("general-purpose"));
    assert_eq!(shape(nested), vec!["request", "exec err", "api error"]);
}

#[test]
fn the_tree_does_not_depend_on_the_order_the_payloads_came_in() {
    let (one, two) = (fixture("swarm-1.json"), fixture("swarm-2.json"));
    let mut t = translate([two.as_str(), one.as_str()], &prices()).unwrap();
    assert_eq!(t.sessions.remove(0).root, swarm().root);
}

#[test]
fn a_subagent_s_parent_may_be_named_by_a_later_record() {
    let rec = |seq: u32, extra: &str| {
        format!(
            r#"{{"attributes":[
                {{"key":"session.id","value":{{"stringValue":"test-s"}}}},
                {{"key":"event.sequence","value":{{"intValue":"{seq}"}}}},
                {{"key":"event.name","value":{{"stringValue":"api_request"}}}}{extra}]}}"#
        )
    };
    let a1 = r#",{"key":"agent_id","value":{"stringValue":"test-a1"}}"#;
    let a2 = r#",{"key":"agent_id","value":{"stringValue":"test-a2"}}"#;
    let parent = r#",{"key":"parent_agent_id","value":{"stringValue":"test-a1"}}"#;
    let payload = format!(
        r#"{{"resourceLogs":[{{"scopeLogs":[{{"logRecords":[{},{},{}]}}]}}]}}"#,
        rec(0, a1),
        rec(1, a2),
        rec(2, &format!("{a2}{parent}")),
    );
    let t = translate([payload.as_str()], &prices()).unwrap();
    let root = &t.sessions[0].root;
    assert_eq!(root.children().len(), 1);
    assert_eq!(root.children()[0].children().len(), 1);
    assert_eq!(
        shape(&root.children()[0].children()[0]),
        vec!["request", "request"]
    );
}

#[test]
fn a_request_keeps_the_reported_cost_and_prices_its_terms_when_it_can() {
    let s = swarm();
    let Element::Request(main) = &s.root.elements()[0] else {
        panic!("the main conversation starts with a request");
    };
    assert_eq!(main.cost.reported, Observed::Known(Cost::from_micros(5000)));
    assert_eq!(main.tokens.cache_write_5m, Observed::Known(200));
    assert_eq!(main.tokens.cache_write_1h, Observed::Known(0));
    let terms = main.cost.terms.known().copied().unwrap();
    assert_eq!(terms.get(TokenKind::CacheWrite).micros(), 750);
    assert_eq!(terms.get(TokenKind::CacheRead).micros(), 300);
    assert_eq!(terms.get(TokenKind::UncachedInput).micros(), 300);
    assert_eq!(terms.get(TokenKind::Output).micros(), 750);

    // Both lifetimes sent as zero under a total that is not: not known.
    let Element::Request(explore) = &s.root.children()[0].elements()[0] else {
        panic!("the subagent starts with a request");
    };
    assert_eq!(explore.tokens.cache_write_5m, Observed::Missing);
    assert_eq!(explore.cost.terms, Observed::Missing);
    assert_eq!(
        explore.cost.reported,
        Observed::Known(Cost::from_micros(4000))
    );
}

#[test]
fn what_could_not_be_placed_is_noted() {
    let (one, two) = (fixture("swarm-1.json"), fixture("swarm-2.json"));
    let t = translate([one.as_str(), two.as_str()], &prices()).unwrap();
    let kinds: Vec<&NoteKind> = t.notes.iter().map(|n| &n.kind).collect();
    assert_eq!(
        kinds,
        vec![
            &NoteKind::NoSession,
            &NoteKind::Unpriced(PriceGap::MissingTokens),
            &NoteKind::UnknownTool("TodoWrite".into()),
            &NoteKind::Orphan {
                agent: "test-gone".into()
            },
        ]
    );
    let records: Vec<usize> = t.notes.iter().map(|n| n.record).collect();
    assert_eq!(records, vec![6, 3, 10, 13]);
}

#[test]
fn a_run_spans_when_the_harness_changed_within_the_session() {
    let s = swarm();
    assert_eq!(s.duration_ms, Observed::Known(12_000));
    let made = s.into_run(context(Observed::Missing)).unwrap();
    assert_eq!(made.start_source, HarnessSource::Telemetry);
    assert_eq!(made.run.start().harness, HarnessVersion::new(2, 1, 319));
    assert_eq!(made.run.end().harness, HarnessVersion::new(2, 1, 320));
    assert_eq!(made.run.span(), Span::Spans);
}

#[test]
fn an_older_harness_groups_subagents_by_type_and_needs_the_version_given() {
    let legacy = fixture("legacy.json");
    let t = translate([legacy.as_str()], &prices()).unwrap();
    let s = t.sessions[0].clone();
    assert_eq!(shape(&s.root), vec!["request"]);
    let roles: Vec<&Role> = s.root.children().iter().map(AgentNode::role).collect();
    assert_eq!(
        roles,
        vec![&sub("general-purpose"), &Role::Auxiliary(AuxKind::Other)]
    );
    assert_eq!(shape(&s.root.children()[0]), vec!["request", "request"]);
    assert!(
        t.notes
            .iter()
            .any(|n| n.kind == NoteKind::UnknownSource("title_generation".into()))
    );
    assert_eq!(s.duration_ms, Observed::Known(4_000));

    assert_eq!(
        s.clone().into_run(context(Observed::Missing)),
        Err(IntoRunError::HarnessUnknown)
    );
    let made = s
        .into_run(context(Observed::Known(HarnessVersion::new(2, 1, 300))))
        .unwrap();
    assert_eq!(made.end_source, HarnessSource::Context);
    assert_eq!(made.run.span(), Span::Within);
}

#[test]
fn the_fixtures_carry_no_identifying_data() {
    const IDS: [&str; 6] = [
        "session.id",
        "agent_id",
        "parent_agent_id",
        "request_id",
        "prompt.id",
        "tool_use_id",
    ];
    const NEVER: [&str; 7] = [
        "user.email",
        "user.id",
        "user.account_uuid",
        "user.account_id",
        "organization.id",
        "workspace.host_paths",
        "vcs.repository.url.full",
    ];
    fn walk(value: &serde_json::Value, file: &str) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(key) = map.get("key").and_then(|k| k.as_str()) {
                    assert!(!NEVER.contains(&key), "{file}: {key}");
                    if IDS.contains(&key) {
                        let v = map["value"]["stringValue"].as_str().unwrap_or("");
                        assert!(v.starts_with("test-"), "{file}: {key}={v}");
                    }
                }
                map.values().for_each(|v| walk(v, file));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, file)),
            _ => {}
        }
    }
    for file in ["swarm-1.json", "swarm-2.json", "legacy.json"] {
        let value: serde_json::Value = serde_json::from_str(&fixture(file)).unwrap();
        walk(&value, file);
    }
}
