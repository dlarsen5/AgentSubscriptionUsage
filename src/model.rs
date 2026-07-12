use chrono::{DateTime, Utc};
use serde::Serialize;

/// Normalized usage for one provider, ready for rendering or JSON output.
#[derive(Serialize)]
pub struct ProviderUsage {
    pub provider: &'static str,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
}

/// One rate-limit window (e.g. the 5-hour session or the weekly cap).
/// `used_percent` is None for windows without a hard limit (pure spend);
/// `detail` carries extra context such as dollar amounts.
#[derive(Serialize)]
pub struct Window {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Window {
    pub fn new(label: impl Into<String>, percent: f64, resets_at: Option<DateTime<Utc>>) -> Self {
        Window {
            label: label.into(),
            used_percent: Some(percent),
            resets_at,
            detail: None,
        }
    }
}
