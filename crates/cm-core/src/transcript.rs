use crate::schema::TurnUsage;
use chrono::DateTime;
use serde_json::Value;

/// Parse one transcript JSONL line; returns `None` for non-assistant rows
/// or lines missing a usable `message.usage`.
pub fn parse_assistant_usage(line: &str) -> Option<TurnUsage> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let session_id = v.get("sessionId")?.as_str()?.to_string();
    let ts_ms = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
        .unwrap_or(0);

    let msg = v.get("message")?;
    let turn_uuid = msg
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| v.get("uuid").and_then(Value::as_str))?
        .to_string();
    let model_id = msg.get("model").and_then(Value::as_str).map(String::from);

    let usage = msg.get("usage")?;
    let i = |k: &str| usage.get(k).and_then(Value::as_i64).unwrap_or(0);
    let cache_creation = usage.get("cache_creation");
    let ephemeral = |k: &str| {
        cache_creation
            .and_then(|c| c.get(k))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };

    Some(TurnUsage {
        session_id,
        turn_uuid,
        ts_ms,
        model_id,
        input_tokens: i("input_tokens"),
        output_tokens: i("output_tokens"),
        cache_creation_input_tokens: i("cache_creation_input_tokens"),
        cache_read_input_tokens: i("cache_read_input_tokens"),
        ephemeral_1h_tokens: ephemeral("ephemeral_1h_input_tokens"),
        ephemeral_5m_tokens: ephemeral("ephemeral_5m_input_tokens"),
        service_tier: usage
            .get("service_tier")
            .and_then(Value::as_str)
            .map(String::from),
    })
}
