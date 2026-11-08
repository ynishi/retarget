//! Builders and a fixed-seed generator for the domain's tests. Test-only: the
//! domain's code never draws a random number.
#![cfg(test)]

use std::collections::BTreeMap;

use crate::domain::experiment::{
    AlphaSplit, Arm, Assigned, ConfirmMargin, Margins, ObservedRun, SpecParts, StoppingParts,
    StoppingRule, Threshold, TrustLimits, Window,
};
use crate::domain::lever::{
    ClaimWhen, Direction, Lever, MechanismClaim, Order, Positions, RoleScope, Step, Surface,
};
use crate::domain::mechanism::Mechanism;
use crate::domain::money::{Cost, TermCosts};
use crate::domain::name::{ExperimentId, Positive, Ratio, Share};
use crate::domain::run::{
    AgentNode, Effective, Element, Observed, Outcome, Request, RequestCost, Role, Run, RunParts,
    SizeBand, Snapshot, TaskClass, TokenUse,
};

/// SplitMix64.
pub struct Gen(u64);

impl Gen {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn around(&mut self, mean: u64, spread: u64) -> u64 {
        let low = mean.saturating_sub(spread);
        low + (self.unit() * (2 * spread + 1) as f64) as u64
    }
    pub fn lognormal(&mut self, median: f64, sigma: f64) -> u64 {
        let u1 = self.unit().max(f64::MIN_POSITIVE);
        let u2 = self.unit();
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
        (median * (sigma * z).exp()).round() as u64
    }
}

pub struct Sample {
    cache_write: u64,
    cache_read: u64,
    success: bool,
}

impl Sample {
    pub fn new(cache_write: u64, cache_read: u64, success: bool) -> Self {
        Self {
            cache_write,
            cache_read,
            success,
        }
    }
}

pub fn task_class() -> TaskClass {
    TaskClass {
        project: "repo".parse().unwrap(),
        language: "rust".parse().unwrap(),
        size: SizeBand::Medium,
    }
}

pub fn request(cache_write: u64, output: u64) -> Request {
    let c = Cost::from_micros;
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
            terms: Observed::Known(TermCosts::new(c(cache_write), c(0), c(0), c(output))),
        },
        model: Observed::Missing,
    }
}

pub fn run_parts(outcome: Outcome, requests: Vec<Request>) -> RunParts {
    let snapshot = Snapshot {
        harness: "2.1.0".parse().unwrap(),
        effective: Effective::default(),
    };
    RunParts {
        session: "s".parse().unwrap(),
        task_class: task_class(),
        outcome,
        duration_ms: Observed::Known(1000),
        start: snapshot.clone(),
        end: snapshot,
        root: AgentNode::new(
            Role::Main,
            requests.into_iter().map(Element::Request).collect(),
            vec![],
        ),
    }
}

pub fn ttl_lever() -> Lever {
    Lever::new(
        "cache.off_main_ttl".parse().unwrap(),
        Positions::new(
            Order::Ordinal,
            vec!["5m".parse().unwrap(), "1h".parse().unwrap()],
        )
        .unwrap(),
        Surface::ProjectSettings,
        RoleScope::All,
        vec![MechanismClaim {
            mechanism: Mechanism::CacheExpiry,
            reduced_when: ClaimWhen::Up,
        }],
    )
    .unwrap()
}

pub fn stopping_parts() -> StoppingParts {
    let r = |v| Ratio::new(v).unwrap();
    StoppingParts {
        rho: Positive::new(1.0).unwrap(),
        alpha: AlphaSplit::new(r(0.05), r(0.05), r(0.03)).unwrap(),
        min_runs_per_arm: 20,
        min_successes_per_arm: 10,
        max_runs: 2000,
        max_confirm_runs: 2000,
    }
}

pub fn spec_parts(lever: Lever) -> SpecParts {
    let r = |v| Ratio::new(v).unwrap();
    let items = lever.positions().items().to_vec();
    let (from, step) = match &lever.claims()[0].reduced_when {
        ClaimWhen::Up => (items[0].clone(), Step::Ordinal(Direction::Up)),
        ClaimWhen::Down => (items[1].clone(), Step::Ordinal(Direction::Down)),
        ClaimWhen::Toward(to) => {
            let from = items.iter().find(|p| *p != to).unwrap().clone();
            (from, Step::Nominal(to.clone()))
        }
    };
    SpecParts {
        id: ExperimentId::new(7),
        lever,
        from,
        step,
        threshold: Threshold(r(0.1)),
        confirm_margin: ConfirmMargin(Share::new(0.0).unwrap()),
        stopping: StoppingRule::new(stopping_parts()).unwrap(),
        margins: Margins {
            success_rate: r(0.1),
            duration: r(0.5),
            rework: r(0.5),
        },
        trust: TrustLimits::new(Positive::new(100.0).unwrap(), r(0.2), r(0.2), 20).unwrap(),
        trial_share: r(0.5),
        holdout_share: r(0.2),
        task_class: task_class(),
    }
}

/// A run of a window, assigned to an arm, whose lever took effect.
pub fn observed(window: &Window, arm: Arm, sample: Sample) -> ObservedRun {
    let position = match arm {
        Arm::Treatment => window.treatment().clone(),
        Arm::Control => window.control().clone(),
    };
    let c = Cost::from_micros;
    let request = Request {
        cost: RequestCost {
            reported: Observed::Missing,
            terms: Observed::Known(TermCosts::new(
                c(sample.cache_write),
                c(sample.cache_read),
                c(0),
                // output the cache levers leave alone: the size of the run
                c(300),
            )),
        },
        ..request(0, 0)
    };
    let outcome = if sample.success {
        Outcome::Success
    } else {
        Outcome::Failure
    };
    let mut parts = run_parts(outcome, vec![request]);
    let effective =
        Effective::new([(window.lever().clone(), Observed::Known(position.clone()))].into());
    parts.start.effective = effective.clone();
    parts.end.effective = effective;
    ObservedRun {
        run: Run::new(parts).unwrap(),
        assigned: BTreeMap::from([(
            window.lever().clone(),
            Assigned {
                window: window.id(),
                position,
            },
        )]),
    }
}

/// The same run on another harness version.
pub fn set_harness(observed: &mut ObservedRun, version: &str) {
    let run = &observed.run;
    let version = version.parse().unwrap();
    let parts = RunParts {
        session: run.session().clone(),
        task_class: run.task_class().clone(),
        outcome: run.outcome(),
        duration_ms: run.duration_ms(),
        start: Snapshot {
            harness: version,
            effective: run.start().effective.clone(),
        },
        end: Snapshot {
            harness: version,
            effective: run.end().effective.clone(),
        },
        root: run.root().clone(),
    };
    observed.run = Run::new(parts).unwrap();
}
