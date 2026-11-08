//! Drives the product end to end with simulated runs, as the app will, and
//! prints what a person would see day by day: the patterns found, the
//! technique shown with what it is expected to do, the stages it goes
//! through, the reading so far, and what is put back.
//!
//! Run with `cargo run -p retarget-core --example simulate`.

use std::collections::BTreeMap;
use std::error::Error;

use retarget_core::domain::assign::{Assigned, ObservedRun};
use retarget_core::domain::lever::{
    ClaimWhen, Lever, MechanismClaim, Order, Positions, RoleScope, Surface,
};
use retarget_core::domain::measure::{
    AlphaSplit, Margins, StoppingParts, StoppingRule, TrustLimits,
};
use retarget_core::domain::mechanism::Mechanism;
use retarget_core::domain::money::{Cost, TermCosts};
use retarget_core::domain::name::{Digest, LeverKey, PositionId, Positive, Ratio};
use retarget_core::domain::product::adoption::AdoptionState;
use retarget_core::domain::product::catalogue::{
    Catalogue, Expected, FitSpec, OnUndecided, Rollout, Source, Stage, Technique, TechniqueParts,
};
use retarget_core::domain::product::finding::{OFF_MAIN_CACHE_REWRITES, public_findings};
use retarget_core::domain::product::fit::{Basis, Reading};
use retarget_core::domain::product::portfolio::{Portfolio, PortfolioCommand, PortfolioEvent};
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

fn catalogue() -> R<Catalogue> {
    let r = |v| Ratio::new(v);
    let technique = Technique::new(TechniqueParts {
        id: "subagent-ttl-1h".parse()?,
        pattern: OFF_MAIN_CACHE_REWRITES.parse()?,
        lever: ttl_lever()?,
        to: pos("1h")?,
        mechanisms: [Mechanism::CacheExpiry].into(),
        expected: Expected::Range(-0.20, -0.05),
        contraindications: vec![],
        fit: FitSpec {
            margin: r(0.05)?,
            holdout: r(0.1)?,
            stopping: StoppingRule::new(StoppingParts {
                rho: Positive::new(1.0)?,
                alpha: AlphaSplit::new(r(0.05)?, r(0.05)?, r(0.03)?)?,
                min_runs_per_arm: 15,
                min_successes_per_arm: 10,
                max_runs: 600,
                max_confirm_runs: 600,
            })?,
            trust: TrustLimits::new(Positive::new(100.0)?, r(0.2)?, r(0.2)?, 15)?,
            margins: Margins {
                success_rate: r(0.1)?,
                duration: r(0.5)?,
                rework: r(0.5)?,
            },
            on_undecided: OnUndecided::KeepWatching,
        },
        rollout: Rollout::Partial(r(0.5)?),
        automatic: false,
        source: Source::Lab,
    })?;
    Ok(Catalogue::new(1, vec![technique])?)
}

fn task_class() -> R<TaskClass> {
    Ok(TaskClass {
        project: "demo".parse()?,
        language: "rust".parse()?,
        size: SizeBand::Medium,
    })
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

/// A customer: how the technique's position changes their subagent's cache
/// writes and reads, as multiples of the old position's.
struct Customer {
    name: &'static str,
    writes: f64,
    reads: f64,
}

/// One session: a main conversation and one subagent. A quarter of the cost
/// is the subagent's cache writes, the rest cache reads and output.
fn session(customer: &Customer, lever: &LeverKey, at: &PositionId, rng: &mut Rng) -> R<Run> {
    let treated = at.as_str() == "1h";
    let (w, rd) = if treated {
        (customer.writes, customer.reads)
    } else {
        (1.0, 1.0)
    };
    let base = rng.session_cost();
    let root = AgentNode::new(
        Role::Main,
        vec![Element::Request(request(
            0.0,
            0.35 * base * rd,
            0.40 * base,
        ))],
        vec![AgentNode::new(
            Role::Sub("explorer".parse()?),
            vec![Element::Request(request(0.25 * base * w, 0.0, 0.0))],
            vec![],
        )],
    );
    let effective = Effective::new([(lever.clone(), Observed::Known(at.clone()))].into());
    let snapshot = Snapshot {
        harness: "2.1.319".parse()?,
        effective,
    };
    Ok(Run::new(RunParts {
        session: format!("s{}", rng.next()).parse()?,
        task_class: task_class()?,
        outcome: if rng.unit() < 0.85 {
            Outcome::Success
        } else {
            Outcome::Failure
        },
        duration_ms: Observed::Known((600_000.0 * (0.5 * rng.normal()).exp()) as u64),
        start: snapshot.clone(),
        end: snapshot,
        root,
    })?)
}

fn describe(reading: &Reading) -> String {
    let expected = match reading.expected {
        Expected::Unproven => "not proven yet".to_string(),
        Expected::Range(low, high) => format!("{:+.0}%..{:+.0}%", low * 100.0, high * 100.0),
    };
    let stage = match reading.state {
        AdoptionState::Shown => "shown, waiting for you".to_string(),
        AdoptionState::Applying(Stage::Partial(s)) => {
            format!("on {:.0}% of sessions", s.get() * 100.0)
        }
        AdoptionState::Applying(_) => "on all sessions but a holdout".to_string(),
        AdoptionState::Kept => "kept, watched against a holdout".to_string(),
    };
    let measure = match reading.basis {
        Basis::CostTerms => "on the cache bill",
        Basis::Objective => "per successful session",
    };
    let seen = match reading.observed {
        Some(change) => format!(
            "so far {:+.0}% {measure} ({} vs {} sessions)",
            change.get() * 100.0,
            reading.counts.treatment,
            reading.counts.control
        ),
        None => "nothing measured yet".to_string(),
    };
    format!(
        "{} · {stage} · expected {expected} · {seen}",
        reading.technique
    )
}

fn day_by_day(customer: Customer, seed: u64, per_day: usize, days: usize) -> R<()> {
    println!("\n== {} ({per_day} sessions a day) ==", customer.name);
    let mut rng = Rng(seed);
    let lever: LeverKey = "cache.off_main_ttl".parse()?;
    let mut at = pos("5m")?;
    let mut file = digest_of(&at);
    let mut portfolio = Portfolio::new("demo".parse()?);
    portfolio.execute(PortfolioCommand::LoadCatalogue(Box::new(catalogue()?)))?;
    let mut recent: Vec<Run> = Vec::new();
    let mut allowed = false;
    for day in 1..=days {
        let mut notices = Vec::new();
        for _ in 0..per_day {
            // put back what the portfolio says to put back
            for (l, to) in portfolio
                .places()
                .pending_put_back()
                .into_iter()
                .map(|(l, to)| (l.clone(), to.clone()))
                .collect::<Vec<_>>()
            {
                portfolio.execute(PortfolioCommand::CheckPutBack(l.clone(), Some(file)))?;
                at = to.clone();
                file = digest_of(&at);
                portfolio.execute(PortfolioCommand::PutBackWritten(l.clone()))?;
                notices.push(format!("{l} put back to {to}"));
            }
            let window = portfolio.windows().first().map(|w| (*w).clone());
            let mut assigned = BTreeMap::new();
            if let Some(window) = window {
                let position = if rng.unit() < window.p() {
                    window.treatment().clone()
                } else {
                    window.control().clone()
                };
                portfolio.execute(PortfolioCommand::PrepareWrite(lever.clone(), Some(file)))?;
                at = position.clone();
                file = digest_of(&at);
                portfolio.execute(PortfolioCommand::Written(lever.clone(), file))?;
                assigned.insert(
                    lever.clone(),
                    Assigned {
                        window: window.id(),
                        position,
                    },
                );
            }
            let run = session(&customer, &lever, &at, &mut rng)?;
            recent.push(run.clone());
            portfolio.execute(PortfolioCommand::Record(Box::new(ObservedRun {
                run,
                assigned,
            })))?;
            for event in portfolio.execute(PortfolioCommand::Evaluate)? {
                match event {
                    PortfolioEvent::StageAdvanced(_, Stage::Full, _) => {
                        notices.push("fits so far: moved to all sessions but a holdout".into())
                    }
                    PortfolioEvent::AdoptionKept(_) => notices.push("fits: kept".into()),
                    PortfolioEvent::AdoptionUnfit(_, reason) => {
                        notices.push(format!("does not fit you ({reason:?}): putting it back"))
                    }
                    _ => {}
                }
            }
        }
        // once a day, look for patterns in the last day's sessions
        let current = BTreeMap::from([(lever.clone(), at.clone())]);
        let findings = public_findings(&recent);
        recent.clear();
        for event in portfolio.execute(PortfolioCommand::Found(findings, current, day as u128))? {
            if let PortfolioEvent::AdoptionShown(..) = event {
                notices.push("found: subagent cache written again and again".into());
            }
        }
        // the person allows a shown technique the next day
        if !allowed
            && day > 1
            && let Some(reading) = portfolio
                .readings()
                .into_iter()
                .find(|r| r.state == AdoptionState::Shown)
        {
            portfolio.execute(PortfolioCommand::Allow(reading.technique.clone()))?;
            notices.push(format!("you allowed {}", reading.technique));
            allowed = true;
        }
        for n in &notices {
            println!("day {day:>2} │ notice: {n}");
        }
        let readings = portfolio.readings();
        let line = if readings.is_empty() {
            "nothing applied".to_string()
        } else {
            readings
                .iter()
                .map(describe)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        println!("day {day:>2} │ status: {line}");
    }
    Ok(())
}

fn main() -> R<()> {
    day_by_day(
        Customer {
            name: "a customer the technique fits (writes -60%)",
            writes: 0.4,
            reads: 1.05,
        },
        1,
        15,
        21,
    )?;
    day_by_day(
        Customer {
            name: "a customer it does not fit (writes -60%, reads x2.5)",
            writes: 0.4,
            reads: 2.5,
        },
        2,
        15,
        21,
    )?;
    Ok(())
}
