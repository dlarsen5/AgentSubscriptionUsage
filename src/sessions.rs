use std::collections::{BTreeMap, HashMap, HashSet};
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

/// Inclusive day range that transcript scanners bucket usage into.
#[derive(Clone, Copy)]
struct DateRange {
    start: NaiveDate,
    end: NaiveDate,
}

impl DateRange {
    fn single(day: NaiveDate) -> Self {
        DateRange {
            start: day,
            end: day,
        }
    }

    fn contains(&self, day: NaiveDate) -> bool {
        day >= self.start && day <= self.end
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
    #[serde(skip)]
    by_day: HashMap<NaiveDate, Tokens>,
}

#[derive(Serialize)]
pub struct ModelStat {
    pub model: String,
    pub sessions: u64,
    pub tokens: Tokens,
}

/// Per-day usage totals across all agents, for the trailing history graph.
#[derive(Serialize)]
pub struct DayStat {
    pub date: NaiveDate,
    pub by_provider: BTreeMap<&'static str, Tokens>,
}

impl DayStat {
    pub fn total_score(&self) -> f64 {
        self.by_provider.values().map(Tokens::score).sum()
    }
}

/// Anchor day for "today": overridable via AGENT_USAGE_DATE=YYYY-MM-DD to
/// inspect a past day.
fn anchor_day() -> NaiveDate {
    std::env::var("AGENT_USAGE_DATE")
        .ok()
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Local::now().date_naive())
}

fn scan_all(home: &str, range: DateRange) -> Vec<SessionStat> {
    let mut sessions = Vec::new();
    sessions.extend(scan_claude(home, range));
    sessions.extend(scan_codex(home, range));
    sessions.extend(scan_pi_like(
        &PathBuf::from(format!("{home}/.pi/agent/sessions")),
        "pi",
        range,
    ));
    sessions.extend(scan_pi_like(
        &PathBuf::from(format!("{home}/.omp/agent/sessions")),
        "omp",
        range,
    ));
    sessions.extend(scan_opencode(home, range));
    sessions
}

/// Scan local session transcripts from all agents for today's usage,
/// sorted by estimated limit consumption.
pub fn collect_today(home: &str) -> Vec<SessionStat> {
    let mut sessions = scan_all(home, DateRange::single(anchor_day()));
    sessions.retain(|s| s.tokens.score() > 0.0);
    sessions.sort_by(|a, b| b.tokens.score().total_cmp(&a.tokens.score()));
    sessions
}

/// Per-day usage for the `days` ending today (inclusive), oldest first.
/// Every day in the window is present, so graphs stay continuous.
pub fn collect_daily(home: &str, days: u32) -> Vec<DayStat> {
    let end = anchor_day();
    let start = end - chrono::Days::new(u64::from(days.saturating_sub(1)));
    let range = DateRange { start, end };

    let mut by_day: BTreeMap<NaiveDate, BTreeMap<&'static str, Tokens>> = BTreeMap::new();
    let mut day = start;
    loop {
        by_day.insert(day, BTreeMap::new());
        if day >= end {
            break;
        }
        day = day.succ_opt().expect("date overflow");
    }

    for session in scan_all(home, range) {
        for (date, tokens) in &session.by_day {
            by_day
                .entry(*date)
                .or_default()
                .entry(session.provider)
                .or_default()
                .add(tokens);
        }
    }

    by_day
        .into_iter()
        .map(|(date, by_provider)| DayStat { date, by_provider })
        .collect()
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

/// Files last modified on or after the range start may contain in-range
/// entries (a file can't contain entries newer than its mtime); per-line
/// timestamps do the precise filtering.
fn modified_in_range(path: &Path, range: DateRange) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| DateTime::<Local>::from(t).date_naive() >= range.start)
        .unwrap_or(false)
}

/// Local-time day of a transcript line; lines without a parseable timestamp
/// are attributed to the range end (matching the old "assume today").
fn line_day(line: &Value, range: DateRange) -> NaiveDate {
    line.get("timestamp")
        .and_then(Value::as_str)
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Local).date_naive())
        .unwrap_or(range.end)
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

/// Accumulates one provider file's usage into the per-model / per-day maps.
#[derive(Default)]
struct FileAcc {
    by_model: HashMap<String, Tokens>,
    by_day: HashMap<NaiveDate, Tokens>,
    requests: u64,
}

impl FileAcc {
    fn record(&mut self, day: NaiveDate, model: &str, tokens: Tokens) {
        self.by_model
            .entry(model.to_string())
            .or_default()
            .add(&tokens);
        self.by_day.entry(day).or_default().add(&tokens);
        self.requests += 1;
    }
}

fn scan_claude(home: &str, range: DateRange) -> Vec<SessionStat> {
    let root = PathBuf::from(format!("{home}/.claude/projects"));
    let mut out = Vec::new();
    for project_dir in read_dir_paths(&root) {
        for file in read_dir_paths(&project_dir) {
            if file.extension().is_none_or(|e| e != "jsonl") || !modified_in_range(&file, range) {
                continue;
            }
            if let Some(stat) = scan_claude_file(&file, range) {
                out.push(stat);
            }
        }
    }
    out
}

fn scan_claude_file(path: &Path, range: DateRange) -> Option<SessionStat> {
    let mut acc = FileAcc::default();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut cwd = None;

    for line in read_lines(path)? {
        if cwd.is_none()
            && let Some(c) = line.get("cwd").and_then(Value::as_str)
        {
            cwd = Some(c.to_string());
        }
        let Some(message) = line.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let day = line_day(&line, range);
        if !range.contains(day) {
            continue;
        }
        // Streaming rewrites the same message on multiple lines; count each
        // API response once.
        if let Some(id) = message.get("id").and_then(Value::as_str)
            && !seen_ids.insert(id.to_string())
        {
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
        acc.record(
            day,
            model,
            Tokens {
                input: get("input_tokens"),
                output: get("output_tokens"),
                cache_read: get("cache_read_input_tokens"),
                cache_write: get("cache_creation_input_tokens"),
                cost_usd: 0.0,
            },
        );
    }

    let session_id = path.file_stem()?.to_string_lossy().to_string();
    let project = cwd
        .as_deref()
        .map(last_path_component)
        .unwrap_or_else(|| "unknown".to_string());
    Some(finish_session("claude", project, session_id, acc))
}

fn scan_codex(home: &str, range: DateRange) -> Vec<SessionStat> {
    let root = PathBuf::from(format!("{home}/.codex/sessions"));
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for path in read_dir_paths(&dir) {
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && modified_in_range(&path, range)
                && let Some(stat) = scan_codex_file(&path, range)
            {
                out.push(stat);
            }
        }
    }
    out
}

fn scan_codex_file(path: &Path, range: DateRange) -> Option<SessionStat> {
    let mut acc = FileAcc::default();
    let mut cwd = None;
    let mut session_id = None;
    let mut model = "unknown".to_string();

    for line in read_lines(path)? {
        let Some(payload) = line.get("payload") else {
            continue;
        };
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
                    && let Some(c) = payload.get("cwd").and_then(Value::as_str)
                {
                    cwd = Some(c.to_string());
                }
            }
            Some("event_msg")
                if payload.get("type").and_then(Value::as_str) == Some("token_count") =>
            {
                // Sum per-request deltas (`last_token_usage`) instead of the
                // cumulative total so a session spanning midnight attributes
                // each part to its own day.
                let Some(last) = payload.get("info").and_then(|i| i.get("last_token_usage")) else {
                    continue;
                };
                let day = line_day(&line, range);
                if !range.contains(day) {
                    continue;
                }
                let get = |k: &str| last.get(k).and_then(Value::as_u64).unwrap_or(0);
                let cached = get("cached_input_tokens");
                acc.record(
                    day,
                    &model.clone(),
                    Tokens {
                        input: get("input_tokens").saturating_sub(cached),
                        output: get("output_tokens"),
                        cache_read: cached,
                        cache_write: 0,
                        cost_usd: 0.0,
                    },
                );
            }
            _ => {}
        }
    }

    let session_id = session_id.unwrap_or_else(|| {
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });
    let project = cwd
        .as_deref()
        .map(last_path_component)
        .unwrap_or_else(|| "unknown".to_string());
    Some(finish_session("codex", project, session_id, acc))
}

/// pi and oh-my-pi (omp) share the same session format: a `session` header
/// line with the cwd, then `message` lines whose assistant entries carry
/// `usage` with token counts and a computed cost.
fn scan_pi_like(root: &Path, provider: &'static str, range: DateRange) -> Vec<SessionStat> {
    let mut out = Vec::new();
    for project_dir in read_dir_paths(root) {
        for file in read_dir_paths(&project_dir) {
            if file.extension().is_none_or(|e| e != "jsonl") || !modified_in_range(&file, range) {
                continue;
            }
            if let Some(stat) = scan_pi_file(&file, provider, range) {
                out.push(stat);
            }
        }
    }
    out
}

fn scan_pi_file(path: &Path, provider: &'static str, range: DateRange) -> Option<SessionStat> {
    let mut acc = FileAcc::default();
    let mut cwd = None;
    let mut session_id = None;

    for line in read_lines(path)? {
        match line.get("type").and_then(Value::as_str) {
            Some("session") => {
                session_id = line.get("id").and_then(Value::as_str).map(str::to_string);
                cwd = line.get("cwd").and_then(Value::as_str).map(str::to_string);
            }
            Some("message") => {
                let Some(message) = line.get("message") else {
                    continue;
                };
                if message.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let Some(usage) = message.get("usage") else {
                    continue;
                };
                let day = line_day(&line, range);
                if !range.contains(day) {
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
                acc.record(
                    day,
                    model,
                    Tokens {
                        input: get("input"),
                        output: get("output"),
                        cache_read: get("cacheRead"),
                        cache_write: get("cacheWrite"),
                        cost_usd: cost,
                    },
                );
            }
            _ => {}
        }
    }

    let session_id = session_id.unwrap_or_else(|| {
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });
    let project = cwd
        .as_deref()
        .map(last_path_component)
        .unwrap_or_else(|| "unknown".to_string());
    Some(finish_session(provider, project, session_id, acc))
}

/// opencode stores one JSON file per message under
/// storage/message/<sessionID>/msg_*.json; assistant messages carry
/// `tokens`, `cost`, `modelID` and the session cwd.
fn scan_opencode(home: &str, range: DateRange) -> Vec<SessionStat> {
    let root = PathBuf::from(format!("{home}/.local/share/opencode/storage/message"));
    let mut out = Vec::new();
    for session_dir in read_dir_paths(&root) {
        // Message files are written once, so the dir mtime tracks the last
        // message; skip sessions untouched within the range.
        if !session_dir.is_dir() || !modified_in_range(&session_dir, range) {
            continue;
        }
        let mut acc = FileAcc::default();
        let mut cwd = None;
        for file in read_dir_paths(&session_dir) {
            if file.extension().is_none_or(|e| e != "json") || !modified_in_range(&file, range) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Ok(msg) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if msg.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let day = msg
                .get("time")
                .and_then(|t| t.get("created"))
                .and_then(Value::as_i64)
                .and_then(DateTime::<chrono::Utc>::from_timestamp_millis)
                .map(|dt| dt.with_timezone(&Local).date_naive())
                .unwrap_or(range.end);
            if !range.contains(day) {
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
                .unwrap_or("unknown")
                .to_string();
            let tokens = &msg["tokens"];
            let get = |k: &str| tokens.get(k).and_then(Value::as_u64).unwrap_or(0);
            let cache = |k: &str| {
                tokens
                    .pointer(&format!("/cache/{k}"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            };
            acc.record(
                day,
                &model,
                Tokens {
                    input: get("input"),
                    // reasoning tokens are billed as output
                    output: get("output") + get("reasoning"),
                    cache_read: cache("read"),
                    cache_write: cache("write"),
                    cost_usd: msg.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
                },
            );
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
        out.push(finish_session("opencode", project, session_id, acc));
    }
    out
}

fn finish_session(
    provider: &'static str,
    project: String,
    session_id: String,
    acc: FileAcc,
) -> SessionStat {
    let mut totals = Tokens::default();
    for t in acc.by_model.values() {
        totals.add(t);
    }
    let dominant = acc
        .by_model
        .iter()
        .max_by(|a, b| a.1.score().total_cmp(&b.1.score()))
        .map(|(m, _)| m.clone())
        .unwrap_or_else(|| "unknown".to_string());
    SessionStat {
        provider,
        project,
        session_id,
        model: dominant,
        requests: acc.requests,
        tokens: totals,
        by_model: acc.by_model,
        by_day: acc.by_day,
    }
}

fn read_dir_paths(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::fs;

    fn fixture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("usage_tests").join(format!(
            "{}-{}",
            std::process::id(),
            name
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
    }

    fn single() -> DateRange {
        DateRange::single(day())
    }

    #[test]
    fn claude_dedups_streaming_lines_and_skips_synthetic() {
        let dir = fixture_dir("claude");
        let file = dir.join("aaaa-session.jsonl");
        let assistant = r#"{"type":"assistant","timestamp":"2026-01-15T12:00:00Z","cwd":"/home/u/projx","message":{"id":"m1","role":"assistant","model":"claude-x","usage":{"input_tokens":10,"output_tokens":100,"cache_read_input_tokens":1000,"cache_creation_input_tokens":50}}}"#;
        let synthetic = r#"{"type":"assistant","timestamp":"2026-01-15T12:01:00Z","message":{"id":"m2","model":"<synthetic>","usage":{"input_tokens":9999,"output_tokens":9999}}}"#;
        let other_day = r#"{"type":"assistant","timestamp":"2026-01-10T12:00:00Z","message":{"id":"m3","model":"claude-x","usage":{"input_tokens":7,"output_tokens":7}}}"#;
        // the same message id appears twice (streaming rewrite) — count once
        fs::write(
            &file,
            [assistant, assistant, synthetic, other_day].join("\n"),
        )
        .unwrap();

        let stat = scan_claude_file(&file, single()).unwrap();
        assert_eq!(stat.requests, 1);
        assert_eq!(stat.tokens.input, 10);
        assert_eq!(stat.tokens.output, 100);
        assert_eq!(stat.tokens.cache_read, 1000);
        assert_eq!(stat.tokens.cache_write, 50);
        assert_eq!(stat.project, "projx");
        assert_eq!(stat.model, "claude-x");
    }

    #[test]
    fn codex_sums_deltas_and_splits_cached_input() {
        let dir = fixture_dir("codex");
        let file = dir.join("rollout-x.jsonl");
        let lines = [
            r#"{"type":"session_meta","payload":{"id":"sess-1","cwd":"/home/u/projy"}}"#,
            r#"{"type":"turn_context","payload":{"model":"gpt-x","cwd":"/home/u/projy"}}"#,
            // input_tokens includes cached_input_tokens; scanner must split
            r#"{"type":"event_msg","timestamp":"2026-01-15T12:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":20}}}}"#,
            // rate-limit-only event with no usage info — ignored
            r#"{"type":"event_msg","timestamp":"2026-01-15T12:01:00Z","payload":{"type":"token_count","info":null}}"#,
            // different day — ignored
            r#"{"type":"event_msg","timestamp":"2026-01-10T12:00:00Z","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":500,"cached_input_tokens":0,"output_tokens":500}}}}"#,
        ];
        fs::write(&file, lines.join("\n")).unwrap();

        let stat = scan_codex_file(&file, single()).unwrap();
        assert_eq!(stat.requests, 1);
        assert_eq!(stat.tokens.input, 60);
        assert_eq!(stat.tokens.cache_read, 40);
        assert_eq!(stat.tokens.output, 20);
        assert_eq!(stat.session_id, "sess-1");
        assert_eq!(stat.project, "projy");
        assert_eq!(stat.model, "gpt-x");
    }

    #[test]
    fn pi_reads_assistant_usage_and_cost() {
        let dir = fixture_dir("pi");
        let file = dir.join("2026-01-15T12-00-00-000Z_abc.jsonl");
        let lines = [
            r#"{"type":"session","id":"pi-1","timestamp":"2026-01-15T11:59:00Z","cwd":"/home/u/projz"}"#,
            r#"{"type":"message","timestamp":"2026-01-15T12:00:00Z","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"message","timestamp":"2026-01-15T12:00:30Z","message":{"role":"assistant","model":"m/x","usage":{"input":5,"output":6,"cacheRead":7,"cacheWrite":8,"cost":{"total":0.5}}}}"#,
        ];
        fs::write(&file, lines.join("\n")).unwrap();

        let stat = scan_pi_file(&file, "pi", single()).unwrap();
        assert_eq!(stat.provider, "pi");
        assert_eq!(stat.requests, 1);
        assert_eq!(stat.tokens.input, 5);
        assert_eq!(stat.tokens.output, 6);
        assert_eq!(stat.tokens.cache_read, 7);
        assert_eq!(stat.tokens.cache_write, 8);
        assert!((stat.tokens.cost_usd - 0.5).abs() < 1e-9);
        assert_eq!(stat.session_id, "pi-1");
        assert_eq!(stat.project, "projz");
    }

    #[test]
    fn multi_day_range_buckets_usage_per_day() {
        let dir = fixture_dir("pi-multiday");
        let file = dir.join("2026-01-14T12-00-00-000Z_def.jsonl");
        let lines = [
            r#"{"type":"session","id":"pi-2","timestamp":"2026-01-14T11:59:00Z","cwd":"/home/u/projz"}"#,
            r#"{"type":"message","timestamp":"2026-01-14T12:00:00Z","message":{"role":"assistant","model":"m/x","usage":{"input":10,"output":1,"cacheRead":0,"cacheWrite":0,"cost":{"total":0}}}}"#,
            r#"{"type":"message","timestamp":"2026-01-15T09:00:00Z","message":{"role":"assistant","model":"m/x","usage":{"input":20,"output":2,"cacheRead":0,"cacheWrite":0,"cost":{"total":0}}}}"#,
            // outside the range — dropped
            r#"{"type":"message","timestamp":"2026-01-05T09:00:00Z","message":{"role":"assistant","model":"m/x","usage":{"input":999,"output":999,"cacheRead":0,"cacheWrite":0,"cost":{"total":0}}}}"#,
        ];
        fs::write(&file, lines.join("\n")).unwrap();

        let range = DateRange {
            start: NaiveDate::from_ymd_opt(2026, 1, 14).unwrap(),
            end: day(),
        };
        let stat = scan_pi_file(&file, "pi", range).unwrap();
        assert_eq!(stat.requests, 2);
        assert_eq!(stat.tokens.input, 30);
        let d14 = &stat.by_day[&NaiveDate::from_ymd_opt(2026, 1, 14).unwrap()];
        let d15 = &stat.by_day[&day()];
        assert_eq!(d14.input, 10);
        assert_eq!(d15.input, 20);
        assert_eq!(stat.by_day.len(), 2);
    }

    #[test]
    fn opencode_bills_reasoning_as_output() {
        // scan_opencode filters by real file mtime, so this test uses the
        // actual current date rather than a fixed one.
        let home = fixture_dir("opencode-home");
        let ses = home.join(".local/share/opencode/storage/message/ses_1");
        fs::create_dir_all(&ses).unwrap();
        let now_ms = Utc::now().timestamp_millis();
        let assistant = format!(
            r#"{{"id":"msg1","sessionID":"ses_1","role":"assistant","time":{{"created":{now_ms}}},"modelID":"m-x","path":{{"cwd":"/home/u/projw"}},"cost":0.25,"tokens":{{"input":1,"output":2,"reasoning":3,"cache":{{"read":4,"write":5}}}}}}"#
        );
        fs::write(ses.join("msg1.json"), assistant).unwrap();
        fs::write(ses.join("msg0.json"), r#"{"id":"msg0","role":"user"}"#).unwrap();

        let stats = scan_opencode(
            home.to_str().unwrap(),
            DateRange::single(Local::now().date_naive()),
        );
        assert_eq!(stats.len(), 1);
        let stat = &stats[0];
        assert_eq!(stat.provider, "opencode");
        assert_eq!(stat.requests, 1);
        assert_eq!(stat.tokens.input, 1);
        assert_eq!(stat.tokens.output, 5); // 2 output + 3 reasoning
        assert_eq!(stat.tokens.cache_read, 4);
        assert_eq!(stat.tokens.cache_write, 5);
        assert!((stat.tokens.cost_usd - 0.25).abs() < 1e-9);
        assert_eq!(stat.project, "projw");
        assert_eq!(stat.session_id, "ses_1");
    }

    #[test]
    fn score_ranks_output_heavy_sessions_above_cache_readers() {
        let generator = Tokens {
            output: 40_000,
            ..Default::default()
        };
        let cache_reader = Tokens {
            cache_read: 1_500_000,
            ..Default::default()
        };
        assert!(generator.score() > cache_reader.score());
    }
}
