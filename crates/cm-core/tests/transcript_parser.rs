use cm_core::transcript::parse_assistant_usage;

const FIXTURE: &str = include_str!("fixtures/assistant_lines.jsonl");

#[test]
fn parses_every_real_assistant_line() {
    let mut seen = 0;
    for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
        let t = parse_assistant_usage(line)
            .unwrap_or_else(|| panic!("failed on line: {}", &line[..line.len().min(200)]));
        assert!(!t.session_id.is_empty(), "session_id empty");
        assert!(!t.turn_uuid.is_empty(), "turn_uuid empty");
        assert!(t.ts_ms > 0, "ts_ms should be populated");
        // At least one of the token fields should be populated on a real assistant turn.
        let total = t.input_tokens
            + t.output_tokens
            + t.cache_creation_input_tokens
            + t.cache_read_input_tokens;
        assert!(total > 0, "expected nonzero token total");
        seen += 1;
    }
    assert_eq!(seen, 3, "fixture should contain 3 lines");
}

#[test]
fn ignores_non_assistant_lines() {
    let user_line = r#"{"type":"user","sessionId":"x","message":{"content":"hi"}}"#;
    assert!(parse_assistant_usage(user_line).is_none());
}

#[test]
fn extracts_all_token_fields_from_synthetic_line() {
    let line = r#"{
      "type":"assistant",
      "sessionId":"sess-abc",
      "timestamp":"2026-04-21T12:34:56.789Z",
      "uuid":"outer-uuid",
      "message":{
        "id":"msg_123",
        "model":"claude-opus-4-7",
        "usage":{
          "input_tokens":100,
          "output_tokens":200,
          "cache_creation_input_tokens":300,
          "cache_read_input_tokens":400,
          "service_tier":"standard",
          "cache_creation":{
            "ephemeral_1h_input_tokens":50,
            "ephemeral_5m_input_tokens":25
          }
        }
      }
    }"#;
    let t = parse_assistant_usage(line).expect("should parse");
    assert_eq!(t.session_id, "sess-abc");
    assert_eq!(t.turn_uuid, "msg_123");
    assert_eq!(t.model_id.as_deref(), Some("claude-opus-4-7"));
    assert_eq!(t.input_tokens, 100);
    assert_eq!(t.output_tokens, 200);
    assert_eq!(t.cache_creation_input_tokens, 300);
    assert_eq!(t.cache_read_input_tokens, 400);
    assert_eq!(t.ephemeral_1h_tokens, 50);
    assert_eq!(t.ephemeral_5m_tokens, 25);
    assert_eq!(t.service_tier.as_deref(), Some("standard"));
    // 2026-04-21T12:34:56.789Z -> 1776774896789 ms
    assert_eq!(t.ts_ms, 1776774896789);
}

#[test]
fn falls_back_to_outer_uuid_when_message_id_missing() {
    let line = r#"{
      "type":"assistant",
      "sessionId":"sess",
      "uuid":"fallback-uuid",
      "timestamp":"2026-04-21T00:00:00Z",
      "message":{"model":"x","usage":{"input_tokens":1,"output_tokens":0}}
    }"#;
    let t = parse_assistant_usage(line).unwrap();
    assert_eq!(t.turn_uuid, "fallback-uuid");
}
