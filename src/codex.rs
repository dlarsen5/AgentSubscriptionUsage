use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};

use crate::model::{ProviderUsage, Window};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/codex/usage";

#[derive(Deserialize)]
struct Auth {
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    account_id: Option<String>,
}

#[derive(Deserialize)]
struct UsageResp {
    plan_type: Option<String>,
    rate_limit: Option<RateLimit>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    additional_rate_limits: Vec<AdditionalLimit>,
}

#[derive(Deserialize)]
struct AdditionalLimit {
    limit_name: Option<String>,
    rate_limit: Option<RateLimit>,
}

#[derive(Deserialize)]
struct RateLimit {
    primary_window: Option<RateWindow>,
    secondary_window: Option<RateWindow>,
}

#[derive(Deserialize)]
struct RateWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<i64>,
    reset_at: Option<i64>,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

pub fn fetch(home: &str) -> Result<ProviderUsage, String> {
    let path = format!("{home}/.codex/auth.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("no Codex credentials at {path} ({e})"))?;
    let auth: Auth = serde_json::from_str(&raw).map_err(|e| format!("cannot parse {path}: {e}"))?;
    let tokens = auth
        .tokens
        .ok_or_else(|| format!("{path} has no ChatGPT tokens — is Codex logged in?"))?;

    let mut req = ureq::get(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", tokens.access_token))
        .set("User-Agent", "usage");
    if let Some(acct) = &tokens.account_id {
        req = req.set("chatgpt-account-id", acct);
    }
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(401, _) => {
            "Codex API returned 401 — open `codex` once to refresh the token".to_string()
        }
        ureq::Error::Status(429, _) => {
            "Codex usage API rate-limited this check (429) — try again in a minute".to_string()
        }
        ureq::Error::Status(code, _) => format!("Codex usage API returned HTTP {code}"),
        other => format!("Codex usage API request failed: {other}"),
    })?;
    let usage: UsageResp = resp
        .into_json()
        .map_err(|e| format!("cannot parse Codex usage response: {e}"))?;

    let mut windows = Vec::new();
    if let Some(rl) = &usage.rate_limit {
        push_windows(&mut windows, rl, None);
    }
    for extra in &usage.additional_rate_limits {
        if let Some(rl) = &extra.rate_limit {
            push_windows(&mut windows, rl, extra.limit_name.as_deref());
        }
    }

    Ok(ProviderUsage {
        provider: "codex",
        plan: usage.plan_type,
        windows,
    })
}

fn push_windows(windows: &mut Vec<Window>, rl: &RateLimit, scope: Option<&str>) {
    for win in [&rl.primary_window, &rl.secondary_window]
        .into_iter()
        .flatten()
    {
        let span = humanize_window(win.limit_window_seconds);
        let label = match scope {
            Some(name) => format!("{span} ({name})"),
            None => match span.as_str() {
                "5h" => "Session (5h)".to_string(),
                "Weekly" => "Weekly (all models)".to_string(),
                other => other.to_string(),
            },
        };
        windows.push(Window::new(
            label,
            win.used_percent.unwrap_or(0.0),
            win.reset_at
                .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0)),
        ));
    }
}

fn humanize_window(seconds: Option<i64>) -> String {
    match seconds {
        Some(s) if s == 5 * 3600 => "5h".to_string(),
        Some(s) if s == 7 * 24 * 3600 => "Weekly".to_string(),
        Some(s) if s % 86400 == 0 => format!("{}d", s / 86400),
        Some(s) if s % 3600 == 0 => format!("{}h", s / 3600),
        Some(s) => format!("{s}s"),
        None => "window".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::UsageResp;

    #[test]
    fn usage_response_allows_null_additional_rate_limits() {
        let raw = r#"{
            "plan_type": "pro",
            "rate_limit": null,
            "additional_rate_limits": null
        }"#;

        let usage: UsageResp = serde_json::from_str(raw).unwrap();
        assert!(usage.additional_rate_limits.is_empty());
    }

    #[test]
    fn usage_response_allows_missing_additional_rate_limits() {
        let raw = r#"{
            "plan_type": "pro",
            "rate_limit": null
        }"#;

        let usage: UsageResp = serde_json::from_str(raw).unwrap();
        assert!(usage.additional_rate_limits.is_empty());
    }
}
