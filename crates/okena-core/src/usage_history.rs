//! Persisted usage samples. Storage and chart data live in
//! `okena-agent-usage::history`; the types are here because the AI proxy's
//! wire schema carries them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
    Grok,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LimitSample {
    pub name: String,
    pub used_percent: f64,
    pub window_seconds: f64,
    pub reset_at: Option<f64>,
}

/// Version 1 timestamps are Unix epoch seconds (UTC).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageSample {
    pub version: u32,
    pub recorded_at: f64,
    pub provider: Provider,
    /// Provider account ID or Claude credential directory, never an access token.
    pub source: String,
    pub plan: Option<String>,
    pub limits: Vec<LimitSample>,
}
