//! Drives a ledger end to end with simulated runs, as the app will: assign each
//! run to an arm of the open window, check and write the lever's place, record
//! the run, evaluate, and put a change back when the ledger says so.
//!
//! Run with `cargo run -p retarget-core --example simulate`.

use std::collections::BTreeMap;
use std::error::Error;

use retarget_core::domain::experiment::{
    AlphaSplit, Assigned, ConfirmMargin, ExperimentSpec, Margins, ObservedRun, SpecParts,
    StoppingParts, StoppingRule, Threshold, TrustLimits,
};
use retarget_core::domain::ledger::{Command, Ledger, LedgerEvent, SurrogatePolicy};
use retarget_core::domain::lever::{
    ClaimWhen, Direction, Lever, MechanismClaim, Order, Positions, RoleScope, Step, Surface,
};
use retarget_core::domain::mechanism::Mechanism;
use retarget_core::domain::money::{Cost, TermCosts};
use retarget_core::domain::name::{
    Digest, ExperimentId, LeverKey, PositionId, Positive, Ratio, Share,
};
use retarget_core::domain::run::{
    AgentNode, Effective, Element, Observed, Outcome, Request, RequestCost, Role, Run, RunParts,
    SizeBand, Snapshot, TaskClass, TokenUse,
};

type R<T> = Result<T, Box<dyn Error>>;

/// SplitMix64, so every scenario is the same each time it runs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let u1 = self.unit().max(f64::MIN_POSITIVE);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
    /// A session's cost: median $0.50, heavy tailed.
    fn session_cost(&mut self) -> f64 {
        500_000.0 * (0.8 * self.normal()).exp()
    }
}

/// How a scenario's runs come out, per position of the lever.
struct World {
    name: &'static str,
    lever: Lever,
    from: &'static str,
    step: Step,
    /// (off-main cache writes, cache reads, output) as multiples of the control's,
    /// and the success rate, for the treatment position.
    treatment: (f64, f64, f64, f64),
    /// After how many runs a person edits the lever's place (if ever).
    edit_at: Option<usize>,
}

fn pos(s: &str) -> R<PositionId> {
    Ok(s.parse()?)
}

fn ttl_lever() -> R<Lever> {
    Ok(Lever::new(
        "cache.off_main_ttl".parse()?,
        Positions::new(Order::Ordinal, vec![pos("5m")?, pos("1h")?])?,
        Surface::ProjectSettings,
        RoleScope::All,
        vec![MechanismClaim {
            mechanism: Mechanism::CacheExpiry,
            reduced_when: ClaimWhen::Up,
        }],
    )?)
}

fn effort_lever() -> R<Lever> {
    Ok(Lever::new(
        "reasoning.effort".parse()?,
        Positions::new(Order::Ordinal, vec![pos("low")?, pos("high")?])?,
        Surface::ProjectSettings,
        RoleScope::All,
        vec![MechanismClaim {
            mechanism: Mechanism::ReasoningVolume,
            reduced_when: ClaimWhen::Down,
        }],
    )?)
}

fn task_class() -> R<TaskClass> {
    Ok(TaskClass {
        project: "demo".parse()?,
        language: "rust".parse()?,
        size: SizeBand::Medium,
    })
}

fn spec(world: &World) -> R<ExperimentSpec> {
    let r = |v| Ratio::new(v);
    Ok(ExperimentSpec::new(SpecParts {
        id: ExperimentId::new(1),
        lever: world.lever.clone(),
        from: pos(world.from)?,
        step: world.step.clone(),
        threshold: Threshold(r(0.1)?),
        confirm_margin: ConfirmMargin(Share::new(0.0)?),
        stopping: StoppingRule::new(StoppingParts {
            rho: Positive::new(1.0)?,
            alpha: AlphaSplit::new(r(0.05)?, r(0.05)?, r(0.03)?)?,
            min_runs_per_arm: 20,
            min_successes_per_arm: 10,
            max_runs: 1500,
            max_confirm_runs: 1500,
        })?,
        margins: Margins {
            success_rate: r(0.05)?,
            duration: r(0.3)?,
            rework: r(0.5)?,
        },
        trust: TrustLimits::new(Positive::new(100.0)?, r(0.2)?, r(0.2)?, 15)?,
        trial_share: r(0.5)?,
        holdout_share: r(0.2)?,
        task_class: task_class()?,
    })?)
}

fn digest_of(position: &PositionId) -> Digest {
    let mut bytes = [0u8; 32];
    for (i, b) in position.as_str().bytes().enumerate().take(32) {
        bytes[i] = b;
    }
    Digest::new(bytes)
}

fn request(cache_write: f64, cache_read: f64, output: f64) -> Request {
    let c = |v: f64| Cost::from_micros(v.max(0.0).round() as u64);
    Request {
        tokens: TokenUse {
            uncached_input: Observed::Missing,
            output: Observed::Missing,
            cache_read: Observed::Missing,
            cache_write_5m: Observed::Missing,
            cache_write_1h: Observed::Missing,
        },
        cost: RequestCost {
            reported: Observed::Missing,
            terms: Observed::Known(TermCosts::new(
                c(cache_write),
                c(cache_read),
                c(0.0),
                c(output),
            )),
        },
        model: Observed::Missing,
    }
}

/// One session: a main conversation and one subagent. A quarter of the cost
/// is the subagent's cache writes, the rest cache reads and output.
fn run(world: &World, lever: &LeverKey, position: &PositionId, rng: &mut Rng) -> R<Run> {
    let treated = position.as_str() != world.from;
    let (w, rd, out, success) = if treated {
        world.treatment
    } else {
        (1.0, 1.0, 1.0, 0.85)
    };
    let base = rng.session_cost();
    let main = AgentNode::new(
        Role::Main,
        vec![Element::Request(request(
            0.0,
            0.35 * base * rd,
            0.40 * base * out,
        ))],
        vec![AgentNode::new(
            Role::Sub("explorer".parse()?),
            vec![Element::Request(request(0.25 * base * w, 0.0, 0.0))],
            vec![],
        )],
    );
    let effective = Effective::new([(lever.clone(), Observed::Known(position.clone()))].into());
    let snapshot = Snapshot {
        harness: "2.1.319".parse()?,
        effective,
    };
    Ok(Run::new(RunParts {
        session: format!("s{}", rng.next()).parse()?,
        task_class: task_class()?,
        outcome: if rng.unit() < success {
            Outcome::Success
        } else {
            Outcome::Failure
        },
        duration_ms: Observed::Known((600_000.0 * (0.5 * rng.normal()).exp()) as u64),
        start: snapshot.clone(),
        end: snapshot,
        root: main,
    })?)
}

fn describe(event: &LedgerEvent) -> Option<String> {
    match event {
        LedgerEvent::Decided(_, v) => {
            if std::env::var_os("SIM_DEBUG").is_some() {
                println!("{v:#?}");
            }
            Some(format!("trial: {}", short(&format!("{v:?}"))))
        }
        LedgerEvent::Confirmed(_, c) => Some(format!("confirm: {}", short(&format!("{c:?}")))),
        LedgerEvent::WatchDecided(l, v) => Some(format!("watch {l}: {}", short(&format!("{v:?}")))),
        LedgerEvent::WatchOpened(..) => Some("watch opened (holdout 20%)".into()),
        LedgerEvent::GivenUpAdded(g) => Some(format!("given up: {} -> {}", g.lever, g.position)),
        LedgerEvent::PutBackDue(l, to) => Some(format!("put back due: {l} -> {to}")),
        LedgerEvent::PutBackDone(l) => Some(format!("put back done: {l}")),
        LedgerEvent::ExternalEditFound(l, _) => Some(format!("a person edited {l}: blocked")),
        LedgerEvent::SurrogateHit(m) => Some(format!("terms of {m:?} foretold the bill")),
        LedgerEvent::SurrogateMiss(m) => Some(format!("terms of {m:?} missed the bill")),
        _ => None,
    }
}

fn short(debug: &str) -> String {
    // the verdict's name, and the relative change when there is one
    let name: String = debug.chars().take_while(|c| *c != '(').collect();
    let relative = debug
        .split("RelativeChange(")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|v| v.parse::<f64>().ok());
    let reason = debug
        .strip_prefix("Revert(")
        .and_then(|rest| rest.split(',').next())
        .map(|r| format!(" {r}"))
        .unwrap_or_default();
    match relative {
        Some(r) => format!("{name}{reason} (cost per success {:+.1}%)", r * 100.0),
        None => format!("{name}{reason}"),
    }
}

fn simulate(world: World, seed: u64) -> R<()> {
    println!("\n== {} ==", world.name);
    let mut rng = Rng(seed);
    let mut ledger = Ledger::new("demo".parse()?, SurrogatePolicy { min_misses: 2 });
    let lever = world.lever.key().clone();
    let mut file = digest_of(&pos(world.from)?);
    ledger.execute(Command::Start(Box::new(spec(&world)?), Some(file)))?;
    for n in 1..=4000usize {
        if world.edit_at == Some(n) {
            file = Digest::new([0xEE; 32]);
        }
        // put back what the ledger says to put back
        for (l, to) in ledger
            .pending_put_back()
            .into_iter()
            .map(|(l, to)| (l.clone(), to.clone()))
            .collect::<Vec<_>>()
        {
            ledger.execute(Command::CheckPutBack(l.clone(), Some(file)))?;
            if ledger.pending_put_back().iter().any(|(k, _)| **k == l) {
                file = digest_of(&to);
                print_events(n, &ledger.execute(Command::PutBackWritten(l))?);
            }
        }
        let Some(window) = ledger.windows().first().map(|w| (*w).clone()) else {
            println!("run {n:>4}: nothing left to assign; stopping");
            break;
        };
        let position = if rng.unit() < window.p() {
            window.treatment().clone()
        } else {
            window.control().clone()
        };
        let events = ledger.execute(Command::PrepareWrite(lever.clone(), Some(file)))?;
        if events
            .iter()
            .any(|e| matches!(e, LedgerEvent::ExternalEditFound(..)))
        {
            print_events(n, &events);
            continue;
        }
        file = digest_of(&position);
        ledger.execute(Command::Written(lever.clone(), file))?;
        let observed = ObservedRun {
            run: run(&world, &lever, &position, &mut rng)?,
            assigned: BTreeMap::from([(
                lever.clone(),
                Assigned {
                    window: window.id(),
                    position,
                },
            )]),
        };
        ledger.execute(Command::Record(Box::new(observed)))?;
        if n % 10 == 0 {
            print_events(n, &ledger.execute(Command::Evaluate)?);
        }
        if n >= 2500 && ledger.active().is_none() {
            break;
        }
    }
    Ok(())
}

/// What a person sees, day by day: the status line at the end of each day,
/// and a notice at the start of the next session when something was decided.
/// Only what the ledger exposes is used, so a gap here is a gap of the read side.
fn day_by_day(world: World, seed: u64, runs_per_day: usize, days: usize) -> R<()> {
    println!("\n== {} ({runs_per_day} sessions a day) ==", world.name);
    let mut rng = Rng(seed);
    let mut ledger = Ledger::new("demo".parse()?, SurrogatePolicy { min_misses: 2 });
    let lever = world.lever.key().clone();
    let mut file = digest_of(&pos(world.from)?);
    ledger.execute(Command::Start(Box::new(spec(&world)?), Some(file)))?;
    println!(
        "day   0 │ notice: trying {lever} {} → next step, on half of the sessions",
        world.from
    );
    let mut in_window = 0usize;
    let mut window_id = None;
    let mut last_line = String::new();
    for day in 1..=days {
        let mut notices = Vec::new();
        for _ in 0..runs_per_day {
            for (l, to) in ledger
                .pending_put_back()
                .into_iter()
                .map(|(l, to)| (l.clone(), to.clone()))
                .collect::<Vec<_>>()
            {
                ledger.execute(Command::CheckPutBack(l.clone(), Some(file)))?;
                file = digest_of(&to);
                ledger.execute(Command::PutBackWritten(l.clone()))?;
                notices.push(format!("{l} put back to {to}"));
            }
            let Some(window) = ledger.windows().first().map(|w| (*w).clone()) else {
                break;
            };
            if window_id != Some(window.id()) {
                window_id = Some(window.id());
                in_window = 0;
            }
            let position = if rng.unit() < window.p() {
                window.treatment().clone()
            } else {
                window.control().clone()
            };
            let events = ledger.execute(Command::PrepareWrite(lever.clone(), Some(file)))?;
            if events
                .iter()
                .any(|e| matches!(e, LedgerEvent::ExternalEditFound(..)))
            {
                notices.extend(events.iter().filter_map(describe));
                continue;
            }
            file = digest_of(&position);
            ledger.execute(Command::Written(lever.clone(), file))?;
            let observed = ObservedRun {
                run: run(&world, &lever, &position, &mut rng)?,
                assigned: BTreeMap::from([(
                    lever.clone(),
                    Assigned {
                        window: window.id(),
                        position,
                    },
                )]),
            };
            ledger.execute(Command::Record(Box::new(observed)))?;
            in_window += 1;
            notices.extend(
                ledger
                    .execute(Command::Evaluate)?
                    .iter()
                    .filter_map(describe),
            );
        }
        let line = status_line(&ledger, in_window);
        if !notices.is_empty() || line != last_line || day % 7 == 0 {
            for n in &notices {
                println!("day {day:>3} │ notice: {n}");
            }
            println!("day {day:>3} │ status: {line}");
            last_line = line;
        }
        if ledger.active().is_none()
            && ledger.watches().is_empty()
            && ledger.pending_put_back().is_empty()
        {
            println!("day {day:>3} │ (nothing running; retarget is idle until the next proposal)");
            break;
        }
    }
    Ok(())
}

/// The status line, from what the ledger exposes and nothing else.
fn status_line(ledger: &Ledger, in_window: usize) -> String {
    let mut parts = Vec::new();
    if let Some(active) = ledger.active() {
        let spec = active.experiment().spec();
        let what = match active.window().kind() {
            retarget_core::domain::experiment::WindowKind::Trial(_) => "trying",
            retarget_core::domain::experiment::WindowKind::Confirm(_) => "confirming",
            retarget_core::domain::experiment::WindowKind::Watch(_) => "watching",
        };
        parts.push(format!(
            "{what} {} {}→{} · {in_window} sessions",
            spec.lever().key(),
            spec.from(),
            spec.to()
        ));
    }
    for (lever, watch) in ledger.watches() {
        parts.push(format!("kept {lever}={}", watch.experiment().spec().to()));
    }
    if !ledger.blocked().is_empty() {
        parts.push(format!("{} blocked by an edit", ledger.blocked().len()));
    }
    if !ledger.given_up().is_empty() {
        parts.push(format!("{} given up", ledger.given_up().len()));
    }
    if parts.is_empty() {
        "idle".into()
    } else {
        parts.join(" · ")
    }
}

fn print_events(n: usize, events: &[LedgerEvent]) {
    for line in events.iter().filter_map(describe) {
        println!("run {n:>4}: {line}");
    }
}

fn main() -> R<()> {
    if std::env::args().any(|a| a == "ux") {
        day_by_day(
            World {
                name: "cache TTL 5m -> 1h that really saves",
                lever: ttl_lever()?,
                from: "5m",
                step: Step::Ordinal(Direction::Up),
                treatment: (0.4, 1.05, 1.0, 0.85),
                edit_at: None,
            },
            1,
            15,
            150,
        )?;
        day_by_day(
            World {
                name: "effort high -> low that changes nothing",
                lever: effort_lever()?,
                from: "high",
                step: Step::Ordinal(Direction::Down),
                treatment: (1.0, 1.0, 1.0, 0.85),
                edit_at: None,
            },
            3,
            15,
            150,
        )?;
        return Ok(());
    }
    simulate(
        World {
            name: "cache TTL 5m -> 1h saves 60% of subagent cache writes",
            lever: ttl_lever()?,
            from: "5m",
            step: Step::Ordinal(Direction::Up),
            treatment: (0.4, 1.05, 1.0, 0.85),
            edit_at: None,
        },
        1,
    )?;
    simulate(
        World {
            name: "cache TTL cuts writes by 60%, but reads double (saving eaten)",
            lever: ttl_lever()?,
            from: "5m",
            step: Step::Ordinal(Direction::Up),
            treatment: (0.4, 1.6, 1.0, 0.85),
            edit_at: None,
        },
        2,
    )?;
    simulate(
        World {
            name: "effort high -> low changes nothing",
            lever: effort_lever()?,
            from: "high",
            step: Step::Ordinal(Direction::Down),
            treatment: (1.0, 1.0, 1.0, 0.85),
            edit_at: None,
        },
        3,
    )?;
    simulate(
        World {
            name: "effort high -> low halves output but success drops to 70%",
            lever: effort_lever()?,
            from: "high",
            step: Step::Ordinal(Direction::Down),
            treatment: (1.0, 1.0, 0.5, 0.70),
            edit_at: None,
        },
        4,
    )?;
    simulate(
        World {
            name: "a person edits the settings at run 150",
            lever: ttl_lever()?,
            from: "5m",
            step: Step::Ordinal(Direction::Up),
            treatment: (0.4, 1.05, 1.0, 0.85),
            edit_at: Some(150),
        },
        5,
    )?;
    Ok(())
}
