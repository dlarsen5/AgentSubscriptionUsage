mod claude;
mod codex;
mod model;
mod openrouter;
mod render;
mod sessions;

use std::io::IsTerminal;

use model::ProviderUsage;

const HELP: &str = "\
usage — coding-agent subscription usage from the terminal

Reads local OAuth credentials for Claude Code (~/.claude/.credentials.json)
and Codex (~/.codex/auth.json), queries each provider's usage API, and
prints rate-limit utilization and reset times.

USAGE:
    usage [OPTIONS]

Also reports OpenRouter credits/spend when a key is found (pi, opencode),
and scans today's local session transcripts from Claude Code, Codex, pi,
oh-my-pi (omp) and opencode to show top sessions and per-model totals.

OPTIONS:
    --json         emit normalized JSON instead of the terminal view
    --claude       only Claude (limits + sessions)
    --codex        only Codex (limits + sessions)
    --openrouter   only OpenRouter (credits + pi/omp/opencode sessions)
    --no-sessions  skip the local top-sessions / by-model scan
    -h, --help     show this help
";

type Fetcher = fn(&str) -> Result<ProviderUsage, String>;

fn main() {
    // Restore default SIGPIPE so `usage | head` exits quietly instead
    // of panicking on a closed stdout.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let mut json = false;
    let mut scan_sessions = true;
    let mut only: Option<&str> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--json" => json = true,
            "--claude" => only = Some("claude"),
            "--codex" => only = Some("codex"),
            "--openrouter" => only = Some("openrouter"),
            "--no-sessions" => scan_sessions = false,
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }

    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => {
            eprintln!("HOME is not set");
            std::process::exit(1);
        }
    };

    // Only spawn fetchers for providers whose credentials exist locally, so
    // machines without a given agent stay quiet.
    let detected = |path: String| std::path::Path::new(&path).exists();
    let fetchers: Vec<(&str, Fetcher)> = [
        (
            "claude",
            claude::fetch as Fetcher,
            detected(format!("{home}/.claude/.credentials.json")),
        ),
        (
            "codex",
            codex::fetch as Fetcher,
            detected(format!("{home}/.codex/auth.json")),
        ),
        (
            "openrouter",
            openrouter::fetch as Fetcher,
            openrouter::find_key(&home).is_some(),
        ),
    ]
    .into_iter()
    .filter(|(name, _, found)| *found && only.is_none_or(|o| o == *name))
    .map(|(name, fetch, _)| (name, fetch))
    .collect();

    let handles: Vec<_> = fetchers
        .into_iter()
        .map(|(name, fetch)| {
            let home = home.clone();
            (name, std::thread::spawn(move || fetch(&home)))
        })
        .collect();

    // Scan local transcripts on the main thread while the HTTP fetches run.
    let mut top_sessions = if scan_sessions {
        sessions::collect_today(&home)
    } else {
        Vec::new()
    };
    if let Some(o) = only {
        // pi, omp and opencode sessions are billed through OpenRouter keys
        // (or local models), so they group under --openrouter.
        top_sessions.retain(|s| match o {
            "openrouter" => matches!(s.provider, "pi" | "omp" | "opencode"),
            _ => s.provider == o,
        });
    }
    let model_totals = sessions::aggregate_models(&top_sessions);

    let mut results: Vec<(&str, Result<ProviderUsage, String>)> = Vec::new();
    for (name, handle) in handles {
        let result = handle
            .join()
            .unwrap_or_else(|_| Err(format!("{name} fetch thread panicked")));
        results.push((name, result));
    }

    let any_ok = results.iter().any(|(_, r)| r.is_ok());

    if json {
        let providers: Vec<&ProviderUsage> = results
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok())
            .collect();
        let errors: std::collections::BTreeMap<&str, &String> = results
            .iter()
            .filter_map(|(name, r)| r.as_ref().err().map(|e| (*name, e)))
            .collect();
        let out = serde_json::json!({
            "fetched_at": chrono::Utc::now(),
            "providers": providers,
            "errors": errors,
            "sessions_today": top_sessions,
            "models_today": model_totals,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        let mut first = true;
        for (name, result) in &results {
            if !first {
                println!();
            }
            first = false;
            match result {
                Ok(usage) => render::print_provider(usage, color),
                Err(err) => eprintln!("{name}: {err}"),
            }
        }
        render::print_sessions(&top_sessions, &model_totals, color);
    }

    if !any_ok {
        std::process::exit(1);
    }
}
