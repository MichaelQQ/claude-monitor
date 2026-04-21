use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatuslineInput {
    pub session_id: String,
    #[serde(default)]
    pub transcript_path: Option<String>,
    pub model: Model,
    #[serde(default)]
    pub workspace: Option<Workspace>,
    #[serde(default)]
    pub cost: Option<Cost>,
    #[serde(default)]
    pub context_window: Option<ContextWindow>,
    #[serde(default)]
    pub rate_limits: Option<RateLimits>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Model {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Workspace {
    #[serde(default)]
    pub current_dir: Option<String>,
    #[serde(default)]
    pub project_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Cost {
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub total_duration_ms: Option<i64>,
    #[serde(default)]
    pub total_api_duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ContextWindow {
    #[serde(default)]
    pub total_input_tokens: Option<i64>,
    #[serde(default)]
    pub total_output_tokens: Option<i64>,
    #[serde(default)]
    pub context_window_size: Option<i64>,
    #[serde(default)]
    pub used_percentage: Option<f64>,
    #[serde(default)]
    pub remaining_percentage: Option<f64>,
    #[serde(default)]
    pub current_usage: Option<CurrentUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CurrentUsage {
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<i64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RateLimits {
    #[serde(default)]
    pub five_hour: Option<RateLimitWindow>,
    #[serde(default)]
    pub seven_day: Option<RateLimitWindow>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitWindow {
    pub used_percentage: f64,
    pub resets_at: i64,
}

/// Per-turn usage extracted from a transcript JSONL `assistant` line.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TurnUsage {
    pub session_id: String,
    pub turn_uuid: String,
    pub ts_ms: i64,
    pub model_id: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub ephemeral_1h_tokens: i64,
    pub ephemeral_5m_tokens: i64,
    pub service_tier: Option<String>,
}

/// Event published on the live broadcast channel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveEvent {
    Snapshot(Box<StatuslineInput>),
    Turn(TurnUsage),
}
