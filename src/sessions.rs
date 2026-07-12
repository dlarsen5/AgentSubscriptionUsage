use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate};
use serde::Serialize;
use serde_json::Value;

/// Token counts for one session or model. `input` excludes cached reads.
/// `cost_usd` is only non-zero for pay-as-you-go providers that record it.
#[derive(Default, Clone, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost_usd: f64,
}

impl Tokens {
    fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.cost_usd += other.cost_usd;
    }

    /// Rough limit-consumption weight, mirroring typical provider pricing
    /// ratios (cache reads ~0.1x input, cache writes ~1.25x, output ~5x).
    /// Only meaningful for ranking, not as an absolute number.
    pub fn score(&self) -> f64 {
        self.input as f64
            + self.cache_write as f64 * 1.25
            + self.cache_read as f64 * 0.1
            + self.output as f64 * 5.0
    }
}

#[derive(Serialize)]
pub struct SessionStat {
    pub provider: &'static str,
    pub project: String,
    pub session_id: String,
    pub model: String,
    pub requests: u64,
    pub tokens: Tokens,
    #[serde(skip)]
    by_model: HashMap<String, Tokens>,
}

#[derive(Serialize)]
pub struct ModelStat {
    pub model: String,
    pub sessions: u64,
    pub tokens: Tokens,
}

/// Scan local session transcripts from all agents for today's usage,
/// sorted by estimated limit consumption. AGENT_USAGE_DATE=YYYY-MM-DD
/// overrides "today" to inspect a past day.
pub fn collect_today(home: &str) -> Vec<SessionStat> {
    let today = std::env::var("AGENT_USAGE_DATE")
        .ok()
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Local::now().date_naive());
    let mut sessions = Vec::new();
    sessions.extend(scan_claude(home, today));
    sessions.extend(scan_codex(home, today));
    sessions.extend(scan_pi_like(
        &PathBuf::from(format!("{home}/.pi/agent/sessions")),
        "pi",
        today,
    ));
    sessions.extend(scan_pi_like(
        &PathBuf::from(format!("{home}/.omp/agent/sessions")),
        "omp",
        today,
    ));
    sessions.extend(scan_opencode(home, today));
    sessions.retain(|s| s.tokens.score() > 0.0);
    sessions.sort_by(|a, b| b.tokens.score().total_cmp(&a.tokens.score()));
    sessions
}

pub fn aggregate_models(sessions: &[SessionStat]) -> Vec<ModelStat> {
    let mut by_model: HashMap<String, ModelStat> = HashMap::new();
    for s in sessions {
        for (model, tokens) in &s.by_model {
            let entry = by_model.entry(model.clone()).or_insert_with(|| ModelStat {
                model: model.clone(),
                sessions: 0,
                tokens: Tokens::default(),
            });
            entry.sessions += 1;
            entry.tokens.add(tokens);
        }
    }
    let mut models: Vec<ModelStat> = by_model.into_values().collect();
    models.sort_by(|a, b| b.tokens.score().total_cmp(&a.tokens.score()));
    models
}

/// Files modified today (a session started yesterday can still contain
/// today's entries; per-line timestamps do the precise filtering).
fn modified_today(path: &Path, today: NaiveDate) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| DateTime::<Local>::from(t).date_naive() == today)
        .unwrap_or(false)
}

fn is_today(line: &Value, today: NaiveDate) -> bool {
    match line.get("timestamp").and_then(Value::as_str) {
        Some(ts) => DateTime::parse_from_rfc3339(ts)
            .map(|dt| dt.with_timezone(&Local).date_naive() == today)
            .unwrap_or(true),
        None => true,
    }
}

fn last_path_component(p: &str) -> String {
    p.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(p)
        .to_string()
}

fn read_lines(path: &Path) -> Option<impl Iterator<Item = Value>> {
    let file = File::open(path).ok()?;
    Some(
        BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str::<Value>(&l).ok()),
    )
}

fn scan_claude(home: &str, today: NaiveDate) -> Vec<SessionStat> {
    let root = PathBuf::from(format!("{home}/.claude/projects"));
    let mut out = Vec::new();
    for project_dir in read_dir_paths(&root) {
        for file in read_dir_paths(&project_dir) {
            if file.extension().is_none_or(|e| e != "jsonl") || !modified_today(&file, today) {
                continue;
            }
            if let Some(stat) = scan_claude_file(&file, today) {
                out.push(stat);
            }
        }
    }
    out
}

fn scan_claude_file(path: &Path, today: NaiveDate) -> Option<SessionStat> {
    let mut by_model: HashMap<String, Tokens> = HashMap::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut cwd = None;
    let mut requests = 0u64;

    for line in read_lines(path)? {
        if cwd.is_none()
            && let Some(c) = line.get("cwd").and_then(Value::as_str) {
                cwd = Some(c.to_string());
            }
        let Some(message) = line.get("message") else { continue };
        let Some(usage) = message.get("usage") else { continue };
        if !is_today(&line, today) {
            continue;
        }
        // Streaming rewrites the same message on multiple lines; count each
        // API response once.
        if let Some(id) = message.get("id").and_then(Value::as_str)
            && !seen_ids.insert(id.to_string()) {
                continue;
            }
        let model = message
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if model == "<synthetic>" {
            continue;
        }
        let get = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
        by_model.entry(model.to_string()).or_default().add(&Tokens {
            input: get("input_tokens"),
            output: get("output_tokens"),
            cache_read: get("cache_read_input_tokens"),
            cache_write: get("cache_creation_input_tokens"),
            cost_usd: 0.0,
        });
        requests += 1;
    }

    let session_id = path.file_stem()?.to_string_lossy().to_string();
    let project = cwd
        .as_deref()
        .map(last_path_component)
        .unwrap_or_else(|| "unknown".to_string());
    Some(finish_session("claude", project, session_id, requests, by_model))
}

fn scan_codex(home: &str, today: NaiveDate) -> Vec<SessionStat> {
    let root = PathBuf::from(format!("{home}/.codex/sessions"));
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for path in read_dir_paths(&dir) {
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && modified_today(&path, today)
                && let Some(stat) = scan_codex_file(&path, today) {
                    out.push(stat);
                }
        }
    }
    out
}

fn scan_codex_file(path: &Path, today: NaiveDate) -> Option<SessionStat> {
    let mut by_model: HashMap<String, Tokens> = HashMap::new();
    let mut cwd = None;
    let mut session_id = None;
    let mut model = "unknown".to_string();
    let mut requests = 0u64;

    for line in read_lines(path)? {
        let Some(payload) = line.get("payload") else { continue };
        match line.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                session_id = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(c) = payload.get("cwd").and_then(Value::as_str) {
                    cwd = Some(c.to_string());
                }
            }
            Some("turn_context") => {
                if let Some(m) = payload.get("model").and_then(Value::as_str) {
                    model = m.to_string();
                }
                if cwd.is_none()
                    && let Some(c) = payload.get("cwd").and_then(Value::as_str) {
                        cwd = Some(c.to_string());
                    }
            }
            Some("event_msg") if payload.get("type").and_then(Value::as_str)
                == Some("token_count") =>
            {
                // Sum per-request deltas (`last_token_usage`) instead of the
                // cumulative total so a session spanning midnight only counts
                // today's part.
                let Some(last) = payload
                    .get("info")
                    .and_then(|i| i.get("last_token_usage"))
                else {
                    continue;
                };
                if !is_today(&line, today) {
                    continue;
                }
                let get = |k: &str| last.get(k).and_then(Value::as_u64).unwrap_or(0);
                let cached = get("cached_input_tokens");
                requests += 1;
                by_model.entry(model.clone()).or_default().add(&Tokens {
                    input: get("input_tokens").saturating_sub(cached),
                    output: get("output_tokens"),
                    cache_read: cached,
                    cache_write: 0,
                    cost_usd: 0.0,
                });
            }
            _ => {}
        }
    }

    let session_id =
        session_id.unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().to_string());
    let project = cwd
        .as_deref()
        .map(last_path_component)
        .unwrap_or_else(|| "unknown".to_string());
    Some(finish_session("codex", project, session_id, requests, by_model))
}

/// pi and oh-my-pi (omp) share the same session format: a `session` header
/// line with the cwd, then `message` lines whose assistant entries carry
/// `usage` with token counts and a computed cost.
fn scan_pi_like(root: &Path, provider: &'static str, today: NaiveDate) -> Vec<SessionStat> {
    let mut out = Vec::new();
    for project_dir in read_dir_paths(root) {
        for file in read_dir_paths(&project_dir) {
            if file.extension().is_none_or(|e| e != "jsonl") || !modified_today(&file, today) {
                continue;
            }
            if let Some(stat) = scan_pi_file(&file, provider, today) {
                out.push(stat);
            }
        }
    }
    out
}

fn scan_pi_file(path: &Path, provider: &'static str, today: NaiveDate) -> Option<SessionStat> {
    let mut by_model: HashMap<String, Tokens> = HashMap::new();
    let mut cwd = None;
    let mut session_id = None;
    let mut requests = 0u64;

    for line in read_lines(path)? {
        match line.get("type").and_then(Value::as_str) {
            Some("session") => {
                session_id = line.get("id").and_then(Value::as_str).map(str::to_string);
                cwd = line.get("cwd").and_then(Value::as_str).map(str::to_string);
            }
            Some("message") => {
                let Some(message) = line.get("message") else { continue };
                if message.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let Some(usage) = message.get("usage") else { continue };
                if !is_today(&line, today) {
                    continue;
                }
                let model = message
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let get = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
                let cost = usage
                    .get("cost")
                    .and_then(|c| c.get("total"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                by_model.entry(model.to_string()).or_default().add(&Tokens {
                    input: get("input"),
                    output: get("output"),
                    cache_read: get("cacheRead"),
                    cache_write: get("cacheWrite"),
                    cost_usd: cost,
                });
                requests += 1;
            }
            _ => {}
        }
    }

    let session_id = session_id
        .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().to_string());
    let project = cwd
        .as_deref()
        .map(last_path_component)
        .unwrap_or_else(|| "unknown".to_string());
    Some(finish_session(provider, project, session_id, requests, by_model))
}

/// opencode stores one JSON file per message under
/// storage/message/<sessionID>/msg_*.json; assistant messages carry
/// `tokens`, `cost`, `modelID` and the session cwd.
fn scan_opencode(home: &str, today: NaiveDate) -> Vec<SessionStat> {
    let root = PathBuf::from(format!("{home}/.local/share/opencode/storage/message"));
    let mut out = Vec::new();
    for session_dir in read_dir_paths(&root) {
        // Message files are written once, so the dir mtime tracks the last
        // message; skip sessions untouched today.
        if !session_dir.is_dir() || !modified_today(&session_dir, today) {
            continue;
        }
        let mut by_model: HashMap<String, Tokens> = HashMap::new();
        let mut cwd = None;
        let mut requests = 0u64;
        for file in read_dir_paths(&session_dir) {
            if file.extension().is_none_or(|e| e != "json") || !modified_today(&file, today) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&file) else { continue };
            let Ok(msg) = serde_json::from_str::<Value>(&raw) else { continue };
            if msg.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let created_today = msg
                .get("time")
                .and_then(|t| t.get("created"))
                .and_then(Value::as_i64)
                .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                .map(|dt| dt.with_timezone(&Local).date_naive() == today)
                .unwrap_or(true);
            if !created_today {
                continue;
            }
            if cwd.is_none() {
                cwd = msg
                    .pointer("/path/cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            let model = msg
                .get("modelID")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let tokens = &msg["tokens"];
            let get = |k: &str| tokens.get(k).and_then(Value::as_u64).unwrap_or(0);
            let cache = |k: &str| {
                tokens
                    .pointer(&format!("/cache/{k}"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            };
            by_model.entry(model.to_string()).or_default().add(&Tokens {
                input: get("input"),
                // reasoning tokens are billed as output
                output: get("output") + get("reasoning"),
                cache_read: cache("read"),
                cache_write: cache("write"),
                cost_usd: msg.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
            });
            requests += 1;
        }
        let session_id = session_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let project = cwd
            .as_deref()
            .map(last_path_component)
            .unwrap_or_else(|| "unknown".to_string());
        out.push(finish_session("opencode", project, session_id, requests, by_model));
    }
    out
}

fn finish_session(
    provider: &'static str,
    project: String,
    session_id: String,
    requests: u64,
    by_model: HashMap<String, Tokens>,
) -> SessionStat {
    let mut totals = Tokens::default();
    for t in by_model.values() {
        totals.add(t);
    }
    let dominant = by_model
        .iter()
        .max_by(|a, b| a.1.score().total_cmp(&b.1.score()))
        .map(|(m, _)| m.clone())
        .unwrap_or_else(|| "unknown".to_string());
    SessionStat {
        provider,
        project,
        session_id,
        model: dominant,
        requests,
        tokens: totals,
        by_model,
    }
}

fn read_dir_paths(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}
