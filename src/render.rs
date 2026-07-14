use chrono::{DateTime, Datelike, Local, Utc};

use crate::model::ProviderUsage;
use crate::sessions::{DayStat, ModelStat, SessionStat};

const BAR_WIDTH: usize = 20;

pub fn print_provider(usage: &ProviderUsage, color: bool) {
    let title = match usage.provider {
        "claude" => "Claude",
        "codex" => "Codex",
        "cursor" => "Cursor",
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

enum Align {
    Left,
    Right,
}

struct Table {
    headers: Vec<&'static str>,
    aligns: Vec<Align>,
    rows: Vec<Vec<String>>,
}

impl Table {
    fn print(&self, color: bool) {
        let widths: Vec<usize> = self
            .headers
            .iter()
            .enumerate()
            .map(|(i, h)| {
                self.rows
                    .iter()
                    .map(|r| r[i].chars().count())
                    .chain([h.chars().count()])
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let border = |l: &str, m: &str, r: &str| {
            let line = widths
                .iter()
                .map(|w| "─".repeat(w + 2))
                .collect::<Vec<_>>()
                .join(m);
            println!("{}", paint(&format!("{l}{line}{r}"), "2", color));
        };
        let row = |cells: Vec<String>, bold: bool| {
            let sep = paint("│", "2", color);
            let mut out = sep.clone();
            for (i, cell) in cells.iter().enumerate() {
                let w = widths[i];
                let padded = match self.aligns[i] {
                    Align::Left => format!(" {cell:<w$} "),
                    Align::Right => format!(" {cell:>w$} "),
                };
                out.push_str(&if bold {
                    paint(&padded, "1", color)
                } else {
                    padded
                });
                out.push_str(&sep);
            }
            println!("{out}");
        };
        border("┌", "┬", "┐");
        row(self.headers.iter().map(|h| h.to_string()).collect(), true);
        border("├", "┼", "┤");
        for r in &self.rows {
            row(r.clone(), false);
        }
        border("└", "┴", "┘");
    }
}

pub fn print_sessions(sessions: &[SessionStat], models: &[ModelStat], color: bool) {
    if sessions.is_empty() {
        return;
    }
    let shown = &sessions[..sessions.len().min(TOP_SESSIONS)];
    let with_cost = sessions.iter().any(|s| s.tokens.cost_usd >= 0.005);

    println!();
    println!("{}", paint("Top sessions today", "1", color));
    let mut table = Table {
        headers: vec![
            "#", "agent", "project", "model", "reqs", "in", "out", "cache-r", "cache-w",
        ],
        aligns: vec![
            Align::Right,
            Align::Left,
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
        ],
        rows: Vec::new(),
    };
    if with_cost {
        table.headers.push("cost");
        table.aligns.push(Align::Right);
    }
    table.headers.push("session");
    table.aligns.push(Align::Left);
    for (i, s) in shown.iter().enumerate() {
        let mut row = vec![
            (i + 1).to_string(),
            s.provider.to_string(),
            s.project.clone(),
            short_model(&s.model).to_string(),
            s.requests.to_string(),
            fmt_num(s.tokens.input),
            fmt_num(s.tokens.output),
            fmt_num(s.tokens.cache_read),
            fmt_num(s.tokens.cache_write),
        ];
        if with_cost {
            row.push(fmt_cost(s.tokens.cost_usd));
        }
        row.push(s.session_id.chars().take(8).collect());
        table.rows.push(row);
    }
    table.print(color);
    if sessions.len() > TOP_SESSIONS {
        println!("  … and {} more", sessions.len() - TOP_SESSIONS);
    }

    println!();
    println!("{}", paint("Today by model", "1", color));
    let model_cost = models.iter().any(|m| m.tokens.cost_usd >= 0.005);
    let mut table = Table {
        headers: vec!["model", "sessions", "in", "out", "cache-r", "cache-w"],
        aligns: vec![
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
        ],
        rows: Vec::new(),
    };
    if model_cost {
        table.headers.push("cost");
        table.aligns.push(Align::Right);
    }
    for m in models {
        let mut row = vec![
            short_model(&m.model).to_string(),
            m.sessions.to_string(),
            fmt_num(m.tokens.input),
            fmt_num(m.tokens.output),
            fmt_num(m.tokens.cache_read),
            fmt_num(m.tokens.cache_write),
        ];
        if model_cost {
            row.push(fmt_cost(m.tokens.cost_usd));
        }
        table.rows.push(row);
    }
    table.print(color);
}

const HISTORY_BAR_WIDTH: usize = 40;

/// (provider, ANSI color, glyph for no-color output)
const PROVIDER_STYLES: [(&str, &str, char); 5] = [
    ("claude", "33", '█'),
    ("codex", "36", '▓'),
    ("pi", "34", '▒'),
    ("omp", "35", '░'),
    ("opencode", "32", '▪'),
];

pub fn print_history(days: &[DayStat], color: bool) {
    if days.is_empty() {
        return;
    }
    let max = days
        .iter()
        .map(DayStat::total_score)
        .fold(0.0_f64, f64::max);

    println!();
    println!(
        "{}",
        paint(
            &format!("Daily usage — last {} days (weighted tokens)", days.len()),
            "1",
            color,
        )
    );
    if max <= 0.0 {
        println!("  no local usage recorded in this window");
        return;
    }

    for day in days {
        let mut bar = String::new();
        let mut filled = 0usize;
        let mut cum = 0.0;
        for (provider, code, glyph) in PROVIDER_STYLES {
            let Some(tokens) = day.by_provider.get(provider) else {
                continue;
            };
            cum += tokens.score();
            let target = ((cum / max) * HISTORY_BAR_WIDTH as f64).round() as usize;
            let width = target.saturating_sub(filled);
            if width > 0 {
                let seg = if color {
                    "█".repeat(width)
                } else {
                    glyph.to_string().repeat(width)
                };
                bar.push_str(&paint(&seg, code, color));
                filled = target;
            }
        }
        let total = day.total_score();
        let label = if total > 0.0 {
            fmt_num(total as u64)
        } else {
            "—".to_string()
        };
        // Format width counts ANSI escape characters too, so widen the pad
        // by the bar's invisible overhead to keep the totals column aligned.
        let pad = HISTORY_BAR_WIDTH + (bar.chars().count() - filled);
        println!(
            "  {}  {:<pad$}  {}",
            day.date.format("%a %b %d"),
            bar,
            label,
        );
    }

    let legend = PROVIDER_STYLES
        .iter()
        .filter(|(p, _, _)| days.iter().any(|d| d.by_provider.contains_key(p)))
        .map(|(p, code, glyph)| {
            let block = if color { '█' } else { *glyph };
            format!("{} {p}", paint(&block.to_string(), code, color))
        })
        .collect::<Vec<_>>()
        .join("  ");
    if !legend.is_empty() {
        println!("  {legend}");
    }
}

fn fmt_cost(cost: f64) -> String {
    if cost >= 0.005 {
        format!("${cost:.2}")
    } else {
        String::new()
    }
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
