use crate::schema::TurnUsage;

/// USD per token, broken down by bucket. Published Anthropic list prices
/// (per million tokens) divided by 1e6.
#[derive(Debug, Clone, Copy)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
}

impl Price {
    const fn per_million(
        input: f64,
        output: f64,
        cache_read: f64,
        cache_write_5m: f64,
        cache_write_1h: f64,
    ) -> Self {
        Self {
            input: input / 1_000_000.0,
            output: output / 1_000_000.0,
            cache_read: cache_read / 1_000_000.0,
            cache_write_5m: cache_write_5m / 1_000_000.0,
            cache_write_1h: cache_write_1h / 1_000_000.0,
        }
    }
}

/// Return the price table for a model id (as emitted by Claude Code, e.g.
/// `claude-opus-4-7-20260101`, `claude-sonnet-4-6`, `claude-haiku-4-5-20251001`).
///
/// Falls back to `None` for unknown ids — the caller records no estimate.
pub fn price_for(model_id: &str) -> Option<Price> {
    let m = model_id.to_ascii_lowercase();
    if m.contains("opus") {
        return Some(Price::per_million(15.0, 75.0, 1.50, 18.75, 30.0));
    }
    if m.contains("sonnet") {
        return Some(Price::per_million(3.0, 15.0, 0.30, 3.75, 6.0));
    }
    if m.contains("haiku") {
        if m.contains("haiku-3") {
            return Some(Price::per_million(0.80, 4.0, 0.08, 1.0, 1.6));
        }
        return Some(Price::per_million(1.0, 5.0, 0.10, 1.25, 2.0));
    }
    None
}

/// Dollar estimate for one turn. Returns `None` when the model is unknown.
///
/// `cache_creation_input_tokens` is the total for the turn; `ephemeral_1h`
/// and `ephemeral_5m` break that total down. If the breakdown is absent or
/// incomplete, the remainder is billed at the 5-minute cache-write rate
/// (the default ephemeral TTL).
pub fn estimate_cost_usd(t: &TurnUsage) -> Option<f64> {
    let model = t.model_id.as_deref()?;
    let p = price_for(model)?;
    let known = t.ephemeral_1h_tokens + t.ephemeral_5m_tokens;
    let unattributed_5m = (t.cache_creation_input_tokens - known).max(0);
    Some(
        t.input_tokens as f64 * p.input
            + t.output_tokens as f64 * p.output
            + t.cache_read_input_tokens as f64 * p.cache_read
            + (t.ephemeral_5m_tokens + unattributed_5m) as f64 * p.cache_write_5m
            + t.ephemeral_1h_tokens as f64 * p.cache_write_1h,
    )
}
