use cm_core::pricing::{estimate_cost_usd, price_for};
use cm_core::schema::TurnUsage;

fn turn(model: Option<&str>) -> TurnUsage {
    TurnUsage {
        session_id: "s".into(),
        turn_uuid: "u".into(),
        ts_ms: 0,
        model_id: model.map(String::from),
        input_tokens: 1_000,
        output_tokens: 2_000,
        cache_creation_input_tokens: 10_000,
        cache_read_input_tokens: 100_000,
        ephemeral_1h_tokens: 4_000,
        ephemeral_5m_tokens: 6_000,
        service_tier: None,
    }
}

#[test]
fn opus_pricing_matches_published_list() {
    // 1k*$15 + 2k*$75 + 100k*$1.50 + 6k*$18.75 + 4k*$30, all per million.
    let t = turn(Some("claude-opus-4-7-20260101"));
    let c = estimate_cost_usd(&t).unwrap();
    let expected = (1_000.0 * 15.0
        + 2_000.0 * 75.0
        + 100_000.0 * 1.50
        + 6_000.0 * 18.75
        + 4_000.0 * 30.0)
        / 1_000_000.0;
    assert!((c - expected).abs() < 1e-9, "got {c}, want {expected}");
}

#[test]
fn sonnet_family_maps_to_sonnet_rates() {
    assert!(price_for("claude-sonnet-4-6").is_some());
    let t = turn(Some("claude-sonnet-4-6"));
    let c = estimate_cost_usd(&t).unwrap();
    assert!(c > 0.0);
}

#[test]
fn haiku_four_five_uses_haiku_rates() {
    let t = turn(Some("claude-haiku-4-5-20251001"));
    let c = estimate_cost_usd(&t).unwrap();
    // Haiku is cheapest.
    let opus = estimate_cost_usd(&turn(Some("claude-opus-4-7"))).unwrap();
    assert!(c < opus);
}

#[test]
fn unattributed_cache_creation_bills_at_5m_rate() {
    // ephemeral breakdown says 0 but cache_creation_input_tokens is 10k.
    let mut t = turn(Some("claude-sonnet-4-6"));
    t.ephemeral_1h_tokens = 0;
    t.ephemeral_5m_tokens = 0;
    let c = estimate_cost_usd(&t).unwrap();
    let expected = (1_000.0 * 3.0
        + 2_000.0 * 15.0
        + 100_000.0 * 0.30
        + 10_000.0 * 3.75)
        / 1_000_000.0;
    assert!((c - expected).abs() < 1e-9, "got {c}, want {expected}");
}

#[test]
fn unknown_model_returns_none() {
    assert!(estimate_cost_usd(&turn(Some("gpt-4"))).is_none());
    assert!(estimate_cost_usd(&turn(None)).is_none());
}
