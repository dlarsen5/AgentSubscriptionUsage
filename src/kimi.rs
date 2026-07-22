use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::model::{ProviderUsage, Window};

const USAGE_URL: &str = "https://api.kimi.com/coding/v1/usages";
const TOKEN_URL: &str = "https://auth.kimi.com/api/oauth/token";
/// Public client id baked into the Kimi Code CLI's device-code flow.
const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";

/// Kimi Code access tokens only live ~15 minutes, so unlike the other
/// providers a stored token is usually stale. We run the same refresh grant
/// the CLI itself uses and persist the rotated pair back to the credentials
/// file (atomically, 0600) — refresh tokens rotate, so skipping the write
/// would invalidate the CLI's own copy.
pub fn fetch(home: &str) -> Result<ProviderUsage, String> {
    let path = format!("{home}/.kimi-code/credentials/kimi-code.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("no Kimi credentials at {path} ({e})"))?;
    let creds: Value =
        serde_json::from_str(&raw).map_err(|e| format!("cannot parse {path}: {e}"))?;
    let mut access = creds
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path} has no access_token — run `kimi login`"))?
        .to_string();

    let expires_at = creds.get("expires_at").and_then(as_int).unwrap_or(0);
    if expires_at <= Utc::now().timestamp() + 30 {
        let refresh = creds
            .get("refresh_token")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{path} has no refresh_token — run `kimi login`"))?;
        access = refresh_and_persist(&path, refresh, &creds)?;
    }

    let resp = ureq::get(USAGE_URL)
        .set("Authorization", &format!("Bearer {access}"))
        .set("Accept", "application/json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => {
                "Kimi API returned 401 — run `kimi login` to re-authenticate".to_string()
            }
            ureq::Error::Status(429, _) => {
                "Kimi usage API rate-limited this check (429) — try again in a minute".to_string()
            }
            ureq::Error::Status(code, _) => format!("Kimi usage API returned HTTP {code}"),
            other => format!("Kimi usage API request failed: {other}"),
        })?;
    let usage: Value = resp
        .into_json()
        .map_err(|e| format!("cannot parse Kimi usage response: {e}"))?;

    Ok(parse(&usage))
}

fn refresh_and_persist(path: &str, refresh_token: &str, old: &Value) -> Result<String, String> {
    let resp = ureq::post(TOKEN_URL)
        .set("Accept", "application/json")
        .send_form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .map_err(|e| match e {
            ureq::Error::Status(400 | 401, _) => {
                "Kimi OAuth refresh was rejected — run `kimi login` to re-authenticate".to_string()
            }
            ureq::Error::Status(code, _) => format!("Kimi OAuth refresh returned HTTP {code}"),
            other => format!("Kimi OAuth refresh failed: {other}"),
        })?;
    let tok: Value = resp
        .into_json()
        .map_err(|e| format!("cannot parse Kimi OAuth refresh response: {e}"))?;

    let access = tok
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("Kimi OAuth refresh response missing access_token")?
        .to_string();
    let expires_in = tok.get("expires_in").and_then(as_int).unwrap_or(900);
    let str_or = |v: &Value, key: &str, fallback: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_string()
    };
    let wire = serde_json::json!({
        "access_token": access,
        // A missing rotated token means the old one is still valid.
        "refresh_token": str_or(&tok, "refresh_token", refresh_token),
        "expires_at": Utc::now().timestamp() + expires_in,
        "scope": str_or(&tok, "scope", old.get("scope").and_then(Value::as_str).unwrap_or("kimi-code")),
        "token_type": str_or(&tok, "token_type", "Bearer"),
        "expires_in": expires_in,
    });
    persist(path, &wire)?;
    Ok(access)
}

/// Same write semantics as the Kimi CLI: temp file next to the target,
/// fsync, rename; 0600 so the credentials stay private.
fn persist(path: &str, wire: &Value) -> Result<(), String> {
    use std::io::Write;
    let tmp = format!("{path}.tmp.{}", std::process::id());
    let data = format!("{}\n", serde_json::to_string_pretty(wire).unwrap());
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write = |mut f: std::fs::File| -> std::io::Result<()> {
        f.write_all(data.as_bytes())?;
        f.sync_all()
    };
    opts.open(&tmp)
        .and_then(write)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot update Kimi credentials at {path}: {e}")
        })
}

fn parse(usage: &Value) -> ProviderUsage {
    // membership.level arrives as e.g. "LEVEL_ADVANCED".
    let plan = usage
        .pointer("/user/membership/level")
        .and_then(Value::as_str)
        .map(|l| l.strip_prefix("LEVEL_").unwrap_or(l).to_lowercase());

    let mut windows = Vec::new();

    // `limits` are the short-window meters (e.g. a 300-minute rolling
    // window); the top-level `usage` object is the weekly allowance.
    if let Some(limits) = usage.get("limits").and_then(Value::as_array) {
        for (idx, item) in limits.iter().enumerate() {
            let detail = item.get("detail").unwrap_or(item);
            let label = window_label(item, detail, idx);
            if let Some(w) = usage_window(detail, &label) {
                windows.push(w);
            }
        }
    }
    if let Some(w) = usage
        .get("usage")
        .and_then(|u| usage_window(u, "Weekly limit"))
    {
        windows.push(w);
    }

    ProviderUsage {
        provider: "kimi",
        plan,
        windows,
    }
}

/// One meter → one gauge row: percent from used/limit, reset from
/// `resetTime`, raw counts in the detail column.
fn usage_window(raw: &Value, label: &str) -> Option<Window> {
    let limit = raw.get("limit").and_then(as_int)?;
    let used = raw
        .get("used")
        .and_then(as_int)
        .or_else(|| raw.get("remaining").and_then(as_int).map(|r| limit - r))
        .unwrap_or(0);
    let percent = if limit > 0 {
        used as f64 / limit as f64 * 100.0
    } else {
        0.0
    };
    let resets_at = raw
        .get("resetTime")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let mut w = Window::new(label.to_string(), percent, resets_at);
    w.detail = Some(format!("{used} of {limit}"));
    Some(w)
}

/// Label a `limits[]` entry the way the Kimi CLI does: explicit name if
/// present, otherwise derived from the window duration (300 MINUTE → "5h").
fn window_label(item: &Value, detail: &Value, idx: usize) -> String {
    for key in ["name", "title", "scope"] {
        if let Some(v) = item
            .get(key)
            .or_else(|| detail.get(key))
            .and_then(Value::as_str)
            && !v.is_empty()
        {
            return v.to_string();
        }
    }
    let window = item.get("window").unwrap_or(&Value::Null);
    let duration = window.get("duration").and_then(as_int);
    let unit = window
        .get("timeUnit")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(d) = duration {
        if unit.contains("MINUTE") {
            if d >= 60 && d % 60 == 0 {
                return format!("{}h limit", d / 60);
            }
            return format!("{d}m limit");
        }
        if unit.contains("HOUR") {
            return format!("{d}h limit");
        }
        if unit.contains("DAY") {
            return format!("{d}d limit");
        }
    }
    format!("Limit #{}", idx + 1)
}

/// Kimi encodes counters as JSON strings ("100").
fn as_int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_usages_shape() {
        let usage: Value = serde_json::from_str(
            r#"{
                "user": {"userId": "u1", "membership": {"level": "LEVEL_ADVANCED"}},
                "usage": {"limit": "100", "used": "4", "remaining": "96",
                          "resetTime": "2026-07-27T22:29:00.591Z"},
                "limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                            "detail": {"limit": "100", "used": "1", "remaining": "99",
                                       "resetTime": "2026-07-22T19:29:00.591Z"}}],
                "parallel": {"limit": "30"}
            }"#,
        )
        .unwrap();

        let provider = parse(&usage);
        assert_eq!(provider.provider, "kimi");
        assert_eq!(provider.plan.as_deref(), Some("advanced"));
        assert_eq!(provider.windows.len(), 2);

        let short = &provider.windows[0];
        assert_eq!(short.label, "5h limit");
        assert_eq!(short.used_percent, Some(1.0));
        assert_eq!(short.detail.as_deref(), Some("1 of 100"));
        assert!(short.resets_at.is_some());

        let weekly = &provider.windows[1];
        assert_eq!(weekly.label, "Weekly limit");
        assert_eq!(weekly.used_percent, Some(4.0));
        assert_eq!(weekly.detail.as_deref(), Some("4 of 100"));
    }

    #[test]
    fn tolerates_missing_fields() {
        let provider = parse(&serde_json::json!({}));
        assert_eq!(provider.plan, None);
        assert!(provider.windows.is_empty());

        let provider = parse(&serde_json::json!({
            "usage": {"limit": 50, "remaining": 30}
        }));
        assert_eq!(provider.windows.len(), 1);
        assert_eq!(provider.windows[0].used_percent, Some(40.0));
        assert_eq!(provider.windows[0].detail.as_deref(), Some("20 of 50"));
        assert_eq!(provider.windows[0].resets_at, None);
    }
}
