//! Findings: patterns of waste found in a customer's runs, and the patterns
//! the public catalogue can find without any tuned feature.
//!
//! What finds a pattern is a detector, a port of [`crate::app`]; tuned
//! detectors plug in from outside the repository. The public ones here look
//! only at the cost terms of the runs, so they work on any harness an adapter
//! can translate.

use crate::domain::cost::{CostBreakdown, Segment, TermSet, Work};
use crate::domain::money::TokenKind;
use crate::domain::name::PatternId;
use crate::domain::run::{Observed, Run};

/// A pattern found in some runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// The pattern.
    pub pattern: PatternId,
    /// The share of the runs' cost the pattern accounts for, between 0 and 1.
    pub share: f64,
    /// The runs it was found in.
    pub runs: u32,
}

/// The pattern of a subagent's cache that is written again and again.
pub const OFF_MAIN_CACHE_REWRITES: &str = "off-main-cache-rewrites";
/// The pattern of work redone after failures.
pub const REWORK: &str = "rework";

/// The share of the runs' cost in a set of cells, over the runs whose cells
/// are known.
fn share_of(runs: &[Run], set: &TermSet) -> Option<(f64, u32)> {
    let mut part = 0u64;
    let mut whole = 0u64;
    let mut counted = 0u32;
    for run in runs {
        let Ok(split) = CostBreakdown::of(run) else {
            continue;
        };
        if let (Observed::Known(p), Observed::Known(w)) =
            (split.sum(set), split.sum(&TermSet::all()))
        {
            part = part.saturating_add(p.micros());
            whole = whole.saturating_add(w.micros());
            counted += 1;
        }
    }
    (whole > 0).then(|| (part as f64 / whole as f64, counted))
}

fn found(name: &str, share: f64, runs: u32, at_least: f64, min_runs: u32) -> Option<Finding> {
    if share < at_least || runs < min_runs {
        return None;
    }
    Some(Finding {
        pattern: name.parse().ok()?,
        share,
        runs,
    })
}

/// Off-main cache writes that are a large part of the bill: a subagent's
/// cache expiring and being written again.
pub fn off_main_cache_rewrites(runs: &[Run]) -> Option<Finding> {
    let cells = TermSet::of_segment(Segment::OffMain)
        .intersection(&TermSet::of_kind(TokenKind::CacheWrite))
        .intersection(&TermSet::of_work(Work::First));
    let (share, n) = share_of(runs, &cells)?;
    found(OFF_MAIN_CACHE_REWRITES, share, n, 0.10, 10)
}

/// Work redone after failures that is a large part of the bill.
pub fn rework(runs: &[Run]) -> Option<Finding> {
    let (share, n) = share_of(runs, &TermSet::of_work(Work::Rework))?;
    found(REWORK, share, n, 0.15, 10)
}

/// Every pattern the public catalogue can find.
pub fn public_findings(runs: &[Run]) -> Vec<Finding> {
    [off_main_cache_rewrites(runs), rework(runs)]
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::run::{AgentNode, Element, Outcome, Role};
    use crate::domain::testkit;

    fn run_with_sub_write(write: u64) -> Run {
        let mut parts = testkit::run_parts(Outcome::Success, vec![testkit::request(0, 500)]);
        parts.root = AgentNode::new(
            Role::Main,
            vec![Element::Request(testkit::request(0, 500))],
            vec![AgentNode::new(
                Role::Sub("explorer".parse().unwrap()),
                vec![Element::Request(testkit::request(write, 0))],
                vec![],
            )],
        );
        Run::new(parts).unwrap()
    }

    #[test]
    fn heavy_subagent_cache_writes_are_found() {
        let runs: Vec<_> = (0..12).map(|_| run_with_sub_write(300)).collect();
        let finding = off_main_cache_rewrites(&runs).unwrap();
        assert_eq!(finding.pattern.as_str(), OFF_MAIN_CACHE_REWRITES);
        assert!((finding.share - 0.375).abs() < 1e-9);
        let light: Vec<_> = (0..12).map(|_| run_with_sub_write(10)).collect();
        assert!(off_main_cache_rewrites(&light).is_none());
        assert!(off_main_cache_rewrites(&runs[..5]).is_none());
    }
}
