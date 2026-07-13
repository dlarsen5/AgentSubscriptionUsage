use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::model::{ProviderUsage, Window};

const RPC_BASE: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService";

/// Cursor's dashboard RPCs (ConnectRPC over JSON). The CLI stores a plain
/// JWT in ~/.config/cursor/auth.json; usage lives server-side only — local
/// chat DBs carry no token counts, so Cursor has no session-table rows.
pub fn fetch(home: &str) -> Result<ProviderUsage, String> {
    let path = format!("{home}/.config/cursor/auth.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("no Cursor credentials at {path} ({e})"))?;
    let auth: Value =
        serde_json::from_str(&raw).map_err(|e| format!("cannot parse {path}: {e}"))?;
    let token = auth
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path} has no accessToken — run `cursor-agent login`"))?;

    let usage = rpc(token, "GetCurrentPeriodUsage")?;
    let plan_info = rpc(token, "GetPlanInfo")?;

    Ok(parse(&usage, &plan_info))
}

fn rpc(token: &str, method: &str) -> Result<Value, String> {
    let resp = ureq::post(&format!("{RPC_BASE}/{method}"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("connect-protocol-version", "1")
        .set("User-Agent", "usage")
        .send_string("{}")
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => {
                "Cursor API returned 401 — run `cursor-agent login` to refresh".to_string()
            }
            ureq::Error::Status(429, _) => {
                "Cursor API rate-limited this check (429) — try again in a minute".to_string()
            }
            ureq::Error::Status(code, _) => format!("Cursor {method} returned HTTP {code}"),
            other => format!("Cursor {method} request failed: {other}"),
        })?;
    resp.into_json()
        .map_err(|e| format!("cannot parse Cursor {method} response: {e}"))
}

fn parse(usage: &Value, plan_info: &Value) -> ProviderUsage {
    let plan = plan_info
        .pointer("/planInfo/planName")
        .and_then(Value::as_str)
        .map(str::to_string);

    let cycle_end = usage
        .get("billingCycleEnd")
        .and_then(as_millis)
        .and_then(DateTime::<Utc>::from_timestamp_millis);

    let mut windows = Vec::new();
    if let Some(pu) = usage.get("planUsage") {
        let limit_cents = pu.get("limit").and_then(Value::as_f64).unwrap_or(0.0);
        let remaining_cents = pu
            .get("remaining")
            .and_then(Value::as_f64)
            .unwrap_or(limit_cents);
        let pct = pu
            .get("totalPercentUsed")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let mut w = Window::new("Included usage (month)", pct, cycle_end);
        if limit_cents > 0.0 {
            w.detail = Some(format!(
                "${:.2} of ${:.2}",
                (limit_cents - remaining_cents) / 100.0,
                limit_cents / 100.0,
            ));
        }
        windows.push(w);

        // Auto vs named-model API usage draw from separate meters; only worth
        // a row once something has been used.
        for (key, label) in [
            ("autoPercentUsed", "· auto models"),
            ("apiPercentUsed", "· named API models"),
        ] {
            if let Some(p) = pu.get(key).and_then(Value::as_f64)
                && p > 0.0
            {
                windows.push(Window::new(label, p, cycle_end));
            }
        }
    }

    ProviderUsage {
        provider: "cursor",
        plan,
        windows,
    }
}

/// Cursor encodes millisecond timestamps as JSON strings ("1784392968000").
fn as_millis(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_period_usage_and_plan() {
        let usage: Value = serde_json::from_str(
            r#"{
                "billingCycleStart": "1781800968000",
                "billingCycleEnd": "1784392968000",
                "planUsage": {
                    "remaining": 1500,
                    "limit": 2000,
                    "autoPercentUsed": 25,
                    "apiPercentUsed": 0,
                    "totalPercentUsed": 25
                }
            }"#,
        )
        .unwrap();
        let plan: Value =
            serde_json::from_str(r#"{"planInfo":{"planName":"Pro","price":"$20/mo"}}"#).unwrap();

        let provider = parse(&usage, &plan);
        assert_eq!(provider.plan.as_deref(), Some("Pro"));
        assert_eq!(provider.windows.len(), 2);
        let total = &provider.windows[0];
        assert_eq!(total.used_percent, Some(25.0));
        assert_eq!(total.detail.as_deref(), Some("$5.00 of $20.00"));
        assert_eq!(
            total.resets_at.unwrap().timestamp_millis(),
            1_784_392_968_000
        );
        assert_eq!(provider.windows[1].label, "· auto models");
    }

    #[test]
    fn tolerates_missing_fields() {
        let provider = parse(
            &serde_json::json!({"planUsage": {}}),
            &serde_json::json!({}),
        );
        assert_eq!(provider.plan, None);
        assert_eq!(provider.windows.len(), 1);
        assert_eq!(provider.windows[0].used_percent, Some(0.0));
        assert_eq!(provider.windows[0].detail, None);
    }
}
