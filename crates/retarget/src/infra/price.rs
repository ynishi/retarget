//! Prices per model, to split a request's cost by kind of token.
//!
//! No price is built in: prices change, and each model has its own, so the
//! caller passes the table it holds (where it is read from is the caller's
//! business). A harness's adapter uses it to price the terms of a request
//! ([`retarget_core::domain::money::TermCosts`]); the cost the harness reported
//! is never replaced by it.
//!
//! Prices are integer micro US dollars per million tokens. A term is
//! `tokens × price / 1 000 000`, computed without overflow and rounded half up
//! to a micro dollar; a term too large for a [`Cost`] is refused.

use std::collections::BTreeMap;

use retarget_core::domain::money::{Cost, TermCosts};
use retarget_core::domain::name::ModelId;
use retarget_core::domain::run::{Observed, TokenUse};

/// One model's prices, in micro US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPrices {
    /// Input neither read from nor written to the cache.
    pub uncached_input: u64,
    /// Output.
    pub output: u64,
    /// Input read from the cache.
    pub cache_read: u64,
    /// Input written to the five-minute cache.
    pub cache_write_5m: u64,
    /// Input written to the one-hour cache.
    pub cache_write_1h: u64,
}

/// Why a request's terms could not be priced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PriceGap {
    /// The table has no prices for the request's model, or the model is not
    /// known.
    #[error("no prices for the model")]
    UnknownModel,
    /// A count of tokens the terms need was not sent.
    #[error("a token count is missing")]
    MissingTokens,
    /// A term does not fit in a cost.
    #[error("a term is too large")]
    Overflow,
}

/// The prices of the models the caller knows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PriceTable(BTreeMap<ModelId, ModelPrices>);

impl PriceTable {
    /// A table of the given prices.
    pub fn new(prices: BTreeMap<ModelId, ModelPrices>) -> Self {
        Self(prices)
    }

    /// The terms of one request, when its model is in the table and every
    /// count of tokens was sent. Both cache lifetimes count as writes.
    pub fn terms(
        &self,
        model: &Observed<ModelId>,
        tokens: &TokenUse,
    ) -> Result<TermCosts, PriceGap> {
        let prices = model
            .known()
            .and_then(|m| self.0.get(m))
            .ok_or(PriceGap::UnknownModel)?;
        let count = |o: Observed<u64>| o.known().copied().ok_or(PriceGap::MissingTokens);
        let write_5m = term(count(tokens.cache_write_5m)?, prices.cache_write_5m)?;
        let write_1h = term(count(tokens.cache_write_1h)?, prices.cache_write_1h)?;
        Ok(TermCosts::new(
            write_5m
                .checked_add(write_1h)
                .map_err(|_| PriceGap::Overflow)?,
            term(count(tokens.cache_read)?, prices.cache_read)?,
            term(count(tokens.uncached_input)?, prices.uncached_input)?,
            term(count(tokens.output)?, prices.output)?,
        ))
    }
}

fn term(tokens: u64, per_million: u64) -> Result<Cost, PriceGap> {
    let micros = (u128::from(tokens) * u128::from(per_million) + 500_000) / 1_000_000;
    u64::try_from(micros)
        .map(Cost::from_micros)
        .map_err(|_| PriceGap::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use retarget_core::domain::money::TokenKind;

    fn model() -> ModelId {
        "test-model".parse().unwrap()
    }

    fn table() -> PriceTable {
        PriceTable::new(BTreeMap::from([(
            model(),
            ModelPrices {
                uncached_input: 3_000_000,
                output: 15_000_000,
                cache_read: 300_000,
                cache_write_5m: 3_750_000,
                cache_write_1h: 6_000_000,
            },
        )]))
    }

    fn tokens(n: u64) -> TokenUse {
        TokenUse {
            uncached_input: Observed::Known(n),
            output: Observed::Known(n),
            cache_read: Observed::Known(n),
            cache_write_5m: Observed::Known(n),
            cache_write_1h: Observed::Known(n),
        }
    }

    #[test]
    fn terms_are_priced_per_million_and_writes_add_both_lifetimes() {
        let terms = table()
            .terms(&Observed::Known(model()), &tokens(1_000))
            .unwrap();
        assert_eq!(terms.get(TokenKind::UncachedInput).micros(), 3_000);
        assert_eq!(terms.get(TokenKind::Output).micros(), 15_000);
        assert_eq!(terms.get(TokenKind::CacheRead).micros(), 300);
        assert_eq!(terms.get(TokenKind::CacheWrite).micros(), 3_750 + 6_000);
    }

    #[test]
    fn a_term_rounds_half_up_to_a_micro_dollar() {
        assert_eq!(term(1, 500_000), Ok(Cost::from_micros(1)));
        assert_eq!(term(1, 499_999), Ok(Cost::from_micros(0)));
        assert_eq!(term(u64::MAX, u64::MAX), Err(PriceGap::Overflow));
    }

    #[test]
    fn a_gap_says_why_nothing_was_priced() {
        let t = table();
        assert_eq!(
            t.terms(&Observed::Missing, &tokens(1)),
            Err(PriceGap::UnknownModel)
        );
        assert_eq!(
            PriceTable::default().terms(&Observed::Known(model()), &tokens(1)),
            Err(PriceGap::UnknownModel)
        );
        let mut partial = tokens(1);
        partial.cache_write_1h = Observed::Missing;
        assert_eq!(
            t.terms(&Observed::Known(model()), &partial),
            Err(PriceGap::MissingTokens)
        );
    }
}
