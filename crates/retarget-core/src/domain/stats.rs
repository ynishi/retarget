//! The statistics a verdict rests on.
//!
//! The unit of inference is the run, because runs are what is assigned to arms
//! at random. In a window, runs arrive in order `i = 1..t`; `Z_i` is 1 for the
//! treatment arm, and `p`, the probability of the treatment arm, is known
//! because it was fixed when the window opened.
//!
//! - Every comparison is `a·R_control − b·R_treatment`, where `R` is a ratio of
//!   an arm's means of a numerator and a denominator, each weighted by the
//!   known probability (`1/p` for the treatment, `1/(1−p)` for the control).
//!   The cost per successful run divides cost by success; a plain mean divides
//!   by one, which makes it the Hájek estimate of the mean. The interval comes
//!   from the delta method's influence,
//!   `ψ_i = a·(1−Z_i)/(1−p)·(n_i − R̂_c·d_i)/μ̂_c − b·Z_i/p·(n_i − R̂_t·d_i)/μ̂_t`.
//!   Plugging in `R̂` and `μ̂` leaves a remainder of order `log log t / t`,
//!   which vanishes faster than the width below. The plain Horvitz–Thompson
//!   pseudo-outcome is unbiased too, but its variance grows with the size of
//!   the measure rather than its spread, so even a success rate that never
//!   moves would take thousands of runs to be shown not worse.
//! - Intervals are the asymptotic confidence sequences of Waudby-Smith,
//!   Arbour, Sinha, Kennedy and Ramdas, "Time-uniform central limit theory and
//!   asymptotic confidence sequences" (arXiv 2103.06476): with sample variance
//!   `σ̂²` and a tuning constant `ρ`, the half-width is
//!   `sqrt( 2(t·σ̂²·ρ² + 1) / (t²·ρ²) · ln( sqrt(t·σ̂²·ρ² + 1) / α ) )`.
//!   They stay valid however often they are looked at, in the limit of many
//!   runs; below a minimum number of runs they are not used. A one-sided call
//!   uses the two-sided `α`, which is conservative.
//! - Sample ratio mismatch is tested sequentially with the Beta-binomial
//!   mixture of Lindon and Malek, "Anytime-valid inference for multinomial
//!   count data" (NeurIPS 2022): the Bayes factor against the expected split
//!   is a test martingale, so it may be checked after every run, and the split
//!   is broken once `ln BF ≥ ln(1/α)`.

/// An interval of values, lower end first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    lower: f64,
    upper: f64,
}

impl Interval {
    /// An interval, refused when an end is not a number or the ends are
    /// reversed. An end may be infinite: an interval with too few runs to say
    /// anything is the whole line.
    pub fn new(lower: f64, upper: f64) -> Option<Self> {
        (!lower.is_nan() && !upper.is_nan() && lower <= upper).then_some(Self { lower, upper })
    }
    /// The whole line: nothing is known.
    pub fn unbounded() -> Self {
        Self {
            lower: f64::NEG_INFINITY,
            upper: f64::INFINITY,
        }
    }
    /// The lower end.
    pub fn lower(&self) -> f64 {
        self.lower
    }
    /// The upper end.
    pub fn upper(&self) -> f64 {
        self.upper
    }
}

/// Whether the arms got the share of runs they were assigned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SrmCheck {
    /// Nothing says otherwise.
    Holds,
    /// The split is broken; the log Bayes factor against the expected split.
    Broken(f64),
}

/// The mean and the sample variance of some values.
pub(crate) fn mean_var(values: &[f64]) -> (f64, f64) {
    let t = values.len();
    if t == 0 {
        return (0.0, 0.0);
    }
    let n = t as f64;
    let mean = values.iter().sum::<f64>() / n;
    if t == 1 {
        return (mean, 0.0);
    }
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    (mean, var)
}

/// The asymptotic confidence sequence around a mean of `t` values with sample
/// variance `var`.
pub(crate) fn asymptotic_cs(mean: f64, var: f64, t: usize, rho: f64, alpha: f64) -> Interval {
    if t == 0 || !mean.is_finite() || !var.is_finite() {
        return Interval::unbounded();
    }
    let n = t as f64;
    let v = n * var * rho * rho + 1.0;
    let half = (2.0 * v / (n * n * rho * rho) * (v.sqrt() / alpha).ln()).sqrt();
    Interval::new(mean - half, mean + half).unwrap_or_else(Interval::unbounded)
}

/// The confidence sequence of the mean of some values.
pub(crate) fn cs_of(values: &[f64], rho: f64, alpha: f64) -> Interval {
    let (mean, var) = mean_var(values);
    asymptotic_cs(mean, var, values.len(), rho, alpha)
}

/// ln Γ(x) for x > 0, by the Lanczos approximation (g = 7, nine terms).
pub(crate) fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // reflection
        return (std::f64::consts::PI / (std::f64::consts::PI * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + G + 0.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

fn ln_beta(a: f64, b: f64) -> f64 {
    ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)
}

/// The sequential test of the split between two arms: `n_t` treatment runs,
/// `n_c` control runs, expected treatment share `p`, prior concentration `k`.
pub(crate) fn sequential_srm(n_t: u64, n_c: u64, p: f64, k: f64, alpha: f64) -> SrmCheck {
    let (a, b) = (k * p, k * (1.0 - p));
    let (nt, nc) = (n_t as f64, n_c as f64);
    let log_bf = ln_beta(a + nt, b + nc) - ln_beta(a, b) - (nt * p.ln() + nc * (1.0 - p).ln());
    if log_bf >= (1.0 / alpha).ln() {
        SrmCheck::Broken(log_bf)
    } else {
        SrmCheck::Holds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ln_gamma_matches_known_values() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(ln_gamma(1.0), 0.0));
        assert!(close(ln_gamma(2.0), 0.0));
        assert!(close(ln_gamma(5.0), 24f64.ln()));
        assert!(close(ln_gamma(0.5), std::f64::consts::PI.sqrt().ln()));
    }

    #[test]
    fn the_sequence_narrows_with_more_runs() {
        let w = |t| {
            let i = asymptotic_cs(0.0, 1.0, t, 1.0, 0.05);
            i.upper() - i.lower()
        };
        assert!(w(10) > w(100));
        assert!(w(100) > w(10_000));
        assert_eq!(asymptotic_cs(0.0, 1.0, 0, 1.0, 0.05), Interval::unbounded());
    }

    #[test]
    fn srm_holds_on_the_expected_split_and_breaks_off_it() {
        assert_eq!(sequential_srm(500, 500, 0.5, 100.0, 0.01), SrmCheck::Holds);
        assert!(matches!(
            sequential_srm(800, 200, 0.5, 100.0, 0.01),
            SrmCheck::Broken(_)
        ));
        assert_eq!(sequential_srm(900, 100, 0.9, 100.0, 0.01), SrmCheck::Holds);
    }

    #[test]
    fn intervals_refuse_reversed_ends() {
        assert!(Interval::new(1.0, 0.0).is_none());
        assert!(Interval::new(f64::NAN, 0.0).is_none());
        assert!(Interval::new(0.0, 1.0).is_some());
    }
}
