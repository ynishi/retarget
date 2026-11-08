//! Amounts of money and the kinds of token they are paid for.
//!
//! Money is a whole number of millionths of a US dollar, so that sums are exact
//! and the same in every arm. A sum that does not fit is a broken input, not a
//! bill, and is refused.
//!
//! # Errors
//!
//! [`MoneyError::Overflow`]: a sum went past what a `u64` of micro dollars
//! holds (about 1.8e13 dollars). The input that produced it is wrong.

/// Why an amount could not be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    /// A sum does not fit in micro dollars.
    #[error("a cost does not fit in a u64 of micro dollars")]
    Overflow,
}

/// An amount in micro US dollars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Cost(u64);

impl Cost {
    /// Nothing.
    pub const ZERO: Cost = Cost(0);

    /// An amount from micro dollars.
    pub fn from_micros(micros: u64) -> Self {
        Self(micros)
    }
    /// The amount in micro dollars.
    pub fn micros(self) -> u64 {
        self.0
    }
    /// The sum, refused when it does not fit.
    pub fn checked_add(self, other: Cost) -> Result<Cost, MoneyError> {
        self.0
            .checked_add(other.0)
            .map(Cost)
            .ok_or(MoneyError::Overflow)
    }
}

/// What a share of a request's cost was paid for. Writes to the prompt cache of
/// every lifetime are one kind, because they are moved by the same mechanisms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenKind {
    /// Writing the prompt cache.
    CacheWrite,
    /// Reading the prompt cache.
    CacheRead,
    /// Input that was neither read from nor written to the cache.
    UncachedInput,
    /// Output, reasoning included.
    Output,
}

impl TokenKind {
    /// Every kind, in a fixed order.
    pub const ALL: [TokenKind; 4] = [
        TokenKind::CacheWrite,
        TokenKind::CacheRead,
        TokenKind::UncachedInput,
        TokenKind::Output,
    ];
}

/// A request's cost split by kind of token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermCosts {
    cache_write: Cost,
    cache_read: Cost,
    uncached_input: Cost,
    output: Cost,
}

impl TermCosts {
    /// The four parts of a request's cost.
    pub fn new(cache_write: Cost, cache_read: Cost, uncached_input: Cost, output: Cost) -> Self {
        Self {
            cache_write,
            cache_read,
            uncached_input,
            output,
        }
    }
    /// The part paid for one kind of token.
    pub fn get(&self, kind: TokenKind) -> Cost {
        match kind {
            TokenKind::CacheWrite => self.cache_write,
            TokenKind::CacheRead => self.cache_read,
            TokenKind::UncachedInput => self.uncached_input,
            TokenKind::Output => self.output,
        }
    }
    /// The four parts together.
    pub fn total(&self) -> Result<Cost, MoneyError> {
        TokenKind::ALL
            .iter()
            .try_fold(Cost::ZERO, |sum, kind| sum.checked_add(self.get(*kind)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_are_exact_and_refuse_overflow() {
        let a = Cost::from_micros(u64::MAX - 1);
        assert_eq!(
            a.checked_add(Cost::from_micros(1)),
            Ok(Cost::from_micros(u64::MAX))
        );
        assert_eq!(
            a.checked_add(Cost::from_micros(2)),
            Err(MoneyError::Overflow)
        );
        let terms = TermCosts::new(
            Cost::from_micros(1),
            Cost::from_micros(2),
            Cost::from_micros(3),
            Cost::from_micros(4),
        );
        assert_eq!(terms.total(), Ok(Cost::from_micros(10)));
    }
}
