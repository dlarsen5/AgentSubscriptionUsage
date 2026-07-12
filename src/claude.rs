use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::model::{ProviderUsage, Window};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Deserialize)]
struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    oauth: OauthCreds,
}

#[derive(Deserialize)]
struct OauthCreds {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

#[derive(Deserialize)]
struct UsageResp {
    #[serde(default)]
    limits: Vec<Limit>,
    five_hour: Option<Bucket>,
    seven_day: Option<Bucket>,
    extra_usage: Option<ExtraUsage>,
}

#[derive(Deserialize)]
struct Limit {
    kind: Option<String>,
    percent: Option<f64>,
    resets_at: Option<String>,
    scope: Option<Scope>,
}

#[derive(Deserialize)]
struct Scope {
    model: Option<ScopeModel>,
}

#[derive(Deserialize)]
struct ScopeModel {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct Bucket {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct ExtraUsage {
    is_enabled: Option<bool>,
    utilization: Option<f64>,
}

pub fn fetch(home: &str) -> Result<ProviderUsage, String> {
    let path = format!("{home}/.claude/.credentials.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("no Claude credentials at {path} ({e})"))?;
    let creds: Credentials =
        serde_json::from_str(&raw).map_err(|e| format!("cannot parse {path}: {e}"))?;

    if let Some(exp) = creds.oauth.expires_at
        && exp < Utc::now().timestamp_millis() {
            return Err("Claude OAuth token expired — open `claude` once to refresh it".into());
        }

    let resp = ureq::get(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", creds.oauth.access_token))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => {
                "Claude API returned 401 — open `claude` once to refresh the token".to_string()
            }
            ureq::Error::Status(429, _) => {
                "Claude usage API rate-limited this check (429) — try again in a minute".to_string()
            }
            ureq::Error::Status(code, _) => format!("Claude usage API returned HTTP {code}"),
            other => format!("Claude usage API request failed: {other}"),
        })?;
    let usage: UsageResp = resp
        .into_json()
        .map_err(|e| format!("cannot parse Claude usage response: {e}"))?;

    let mut windows = Vec::new();
    for limit in &usage.limits {
        let label = match limit.kind.as_deref() {
            Some("session") => "Session (5h)".to_string(),
            Some("weekly_all") => "Weekly (all models)".to_string(),
            Some("weekly_scoped") => {
                let model = limit
                    .scope
                    .as_ref()
                    .and_then(|s| s.model.as_ref())
                    .and_then(|m| m.display_name.as_deref())
                    .unwrap_or("scoped");
                format!("Weekly ({model})")
            }
            Some(other) => other.to_string(),
            None => continue,
        };
        windows.push(Window::new(
            label,
            limit.percent.unwrap_or(0.0),
            limit.resets_at.as_deref().and_then(parse_rfc3339),
        ));
    }

    // Older response shape: top-level five_hour / seven_day buckets only.
    if windows.is_empty() {
        for (bucket, label) in [
            (&usage.five_hour, "Session (5h)"),
            (&usage.seven_day, "Weekly (all models)"),
        ] {
            if let Some(b) = bucket {
                windows.push(Window::new(
                    label,
                    b.utilization.unwrap_or(0.0),
                    b.resets_at.as_deref().and_then(parse_rfc3339),
                ));
            }
        }
    }

    if let Some(extra) = &usage.extra_usage
        && extra.is_enabled == Some(true) {
            windows.push(Window::new(
                "Extra usage credits",
                extra.utilization.unwrap_or(0.0),
                None,
            ));
        }

    Ok(ProviderUsage {
        provider: "claude",
        plan: creds.oauth.subscription_type,
        windows,
    })
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
