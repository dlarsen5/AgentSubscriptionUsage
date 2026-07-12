use serde::Deserialize;

use crate::model::{ProviderUsage, Window};

const KEY_URL: &str = "https://openrouter.ai/api/v1/key";
const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";

#[derive(Deserialize)]
struct KeyResp {
    data: KeyData,
}

#[derive(Deserialize)]
struct KeyData {
    usage_daily: Option<f64>,
    usage_weekly: Option<f64>,
    usage_monthly: Option<f64>,
    limit: Option<f64>,
    limit_remaining: Option<f64>,
}

#[derive(Deserialize)]
struct CreditsResp {
    data: CreditsData,
}

#[derive(Deserialize)]
struct CreditsData {
    total_credits: Option<f64>,
    total_usage: Option<f64>,
}

/// pi and opencode both store OpenRouter API keys; return the first found.
pub fn find_key(home: &str) -> Option<String> {
    let candidates = [
        (
            format!("{home}/.pi/agent/auth.json"),
            &["openrouter", "key"][..],
        ),
        (
            format!("{home}/.local/share/opencode/auth.json"),
            &["openrouter", "key"][..],
        ),
    ];
    for (path, pointer) in candidates {
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else { continue };
        let mut cur = &value;
        for p in pointer {
            let Some(next) = cur.get(p) else { break };
            cur = next;
        }
        if let Some(key) = cur.as_str() {
            return Some(key.to_string());
        }
    }
    None
}

pub fn fetch(home: &str) -> Result<ProviderUsage, String> {
    let key = find_key(home).ok_or("no OpenRouter key found (pi / opencode)")?;
    let auth = format!("Bearer {key}");

    let get = |url: &str| {
        ureq::get(url)
            .set("Authorization", &auth)
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(401, _) => {
                    "OpenRouter returned 401 — key revoked or expired".to_string()
                }
                ureq::Error::Status(code, _) => format!("OpenRouter returned HTTP {code}"),
                other => format!("OpenRouter request failed: {other}"),
            })
    };

    let credits: CreditsResp = get(CREDITS_URL)?
        .into_json()
        .map_err(|e| format!("cannot parse OpenRouter credits response: {e}"))?;
    let keyinfo: KeyResp = get(KEY_URL)?
        .into_json()
        .map_err(|e| format!("cannot parse OpenRouter key response: {e}"))?;

    let mut windows = Vec::new();
    let used = credits.data.total_usage.unwrap_or(0.0);
    if let Some(total) = credits.data.total_credits.filter(|t| *t > 0.0) {
        let mut w = Window::new("Credits", used / total * 100.0, None);
        w.detail = Some(format!("${used:.2} of ${total:.2}"));
        windows.push(w);
    } else {
        windows.push(Window {
            label: "Credits used".to_string(),
            used_percent: None,
            resets_at: None,
            detail: Some(format!("${used:.2}")),
        });
    }

    let d = &keyinfo.data;
    if let Some(limit) = d.limit.filter(|l| *l > 0.0) {
        let spent = limit - d.limit_remaining.unwrap_or(limit);
        let mut w = Window::new("Key limit", spent / limit * 100.0, None);
        w.detail = Some(format!("${spent:.2} of ${limit:.2}"));
        windows.push(w);
    }
    windows.push(Window {
        label: "Spend".to_string(),
        used_percent: None,
        resets_at: None,
        detail: Some(format!(
            "today ${:.2} · 7d ${:.2} · 30d ${:.2}",
            d.usage_daily.unwrap_or(0.0),
            d.usage_weekly.unwrap_or(0.0),
            d.usage_monthly.unwrap_or(0.0),
        )),
    });

    Ok(ProviderUsage {
        provider: "openrouter",
        plan: Some("pay-as-you-go".to_string()),
        windows,
    })
}
