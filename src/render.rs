use chrono::{DateTime, Datelike, Local, Utc};

use crate::model::ProviderUsage;
use crate::sessions::{ModelStat, SessionStat, Tokens};

const BAR_WIDTH: usize = 20;

pub fn print_provider(usage: &ProviderUsage, color: bool) {
    let title = match usage.provider {
        "claude" => "Claude",
        "codex" => "Codex",
        "openrouter" => "OpenRouter",
        other => other,
    };
    let plan = usage
        .plan
        .as_deref()
        .map(|p| format!(" ({p})"))
        .unwrap_or_default();
    println!("{}{}", paint(title, "1", color), paint(&plan, "2", color));

    if usage.windows.is_empty() {
        println!("  no rate-limit windows reported");
        return;
    }

    let label_width = usage
        .windows
        .iter()
        .map(|w| w.label.len())
        .max()
        .unwrap_or(0);

    for win in &usage.windows {
        let gauge = match win.used_percent {
            Some(p) => {
                let pct = p.clamp(0.0, 100.0);
                let filled = ((pct / 100.0) * BAR_WIDTH as f64).round() as usize;
                let bar = format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled));
                let code = if pct >= 80.0 {
                    "31" // red
                } else if pct >= 50.0 {
                    "33" // yellow
                } else {
                    "32" // green
                };
                format!("{} {:>4}", paint(&bar, code, color), format!("{p:.0}%"))
            }
            None => String::new(),
        };
        let reset = win
            .resets_at
            .map(|dt| format!("  resets {}", fmt_reset(dt)))
            .unwrap_or_default();
        let detail = win
            .detail
            .as_deref()
            .map(|d| format!("  {d}"))
            .unwrap_or_default();
        println!(
            "  {:<label_width$}  {}{}{}",
            win.label,
            gauge,
            detail,
            paint(&reset, "2", color),
        );
    }
}

const TOP_SESSIONS: usize = 10;

pub fn print_sessions(sessions: &[SessionStat], models: &[ModelStat], color: bool) {
    if sessions.is_empty() {
        return;
    }
    println!();
    println!("{}", paint("Top sessions today", "1", color));
    let shown = &sessions[..sessions.len().min(TOP_SESSIONS)];
    let prov_w = shown.iter().map(|s| s.provider.len()).max().unwrap_or(0);
    let proj_w = shown.iter().map(|s| s.project.len()).max().unwrap_or(0);
    let model_w = shown
        .iter()
        .map(|s| short_model(&s.model).len())
        .max()
        .unwrap_or(0);
    for (i, s) in shown.iter().enumerate() {
        println!(
            "  {:>2}. {:<prov_w$} {:<proj_w$}  {:<model_w$}  {:>3} reqs  {}  {}",
            i + 1,
            s.provider,
            s.project,
            short_model(&s.model),
            s.requests,
            fmt_tokens(&s.tokens),
            paint(&s.session_id.chars().take(8).collect::<String>(), "2", color),
        );
    }
    if sessions.len() > TOP_SESSIONS {
        println!("  … and {} more", sessions.len() - TOP_SESSIONS);
    }

    println!();
    println!("{}", paint("Today by model", "1", color));
    let model_w = models
        .iter()
        .map(|m| short_model(&m.model).len())
        .max()
        .unwrap_or(0);
    for m in models {
        println!(
            "  {:<model_w$}  {:>2} sessions  {}",
            short_model(&m.model),
            m.sessions,
            fmt_tokens(&m.tokens),
        );
    }
}

fn fmt_tokens(t: &Tokens) -> String {
    let cost = if t.cost_usd >= 0.005 {
        format!("  ${:.2}", t.cost_usd)
    } else {
        String::new()
    };
    format!(
        "in {:>6}  out {:>6}  cache-r {:>6}  cache-w {:>6}{}",
        fmt_num(t.input),
        fmt_num(t.output),
        fmt_num(t.cache_read),
        fmt_num(t.cache_write),
        cost,
    )
}

fn fmt_num(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn short_model(model: &str) -> &str {
    let model = model.rsplit('/').next().unwrap_or(model);
    model.strip_prefix("claude-").unwrap_or(model)
}

fn paint(s: &str, code: &str, color: bool) -> String {
    if color && !s.is_empty() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn fmt_reset(dt: DateTime<Utc>) -> String {
    let now = Utc::now();
    let local = dt.with_timezone(&Local);
    let today = Local::now();
    let abs = if local.date_naive() == today.date_naive() {
        local.format("%H:%M").to_string()
    } else if local.year() == today.year() {
        local.format("%a %d %b %H:%M").to_string()
    } else {
        local.format("%Y-%m-%d %H:%M").to_string()
    };
    let delta = dt - now;
    let rel = if delta.num_seconds() <= 0 {
        "now".to_string()
    } else {
        let mins = delta.num_minutes();
        match (mins / 1440, (mins % 1440) / 60, mins % 60) {
            (0, 0, m) => format!("in {m}m"),
            (0, h, m) => format!("in {h}h {m}m"),
            (d, h, _) => format!("in {d}d {h}h"),
        }
    };
    format!("{abs} ({rel})")
}
