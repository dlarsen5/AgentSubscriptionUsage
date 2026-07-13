# agent_usage

Print coding-agent subscription usage in the terminal — no web UI needed.

Reads the credentials your locally installed agents already store on disk,
queries each provider's own usage API for **rate limits**, and scans local
session transcripts for **today's per-session / per-model usage**:

| Provider block | Source |
|---|---|
| Claude (session/weekly limits) | `~/.claude/.credentials.json` → `api.anthropic.com/api/oauth/usage` |
| Codex (session/weekly limits) | `~/.codex/auth.json` → `chatgpt.com/backend-api/codex/usage` |
| OpenRouter (credits & spend) | key from pi/opencode → `openrouter.ai/api/v1/{credits,key}` |
| Top sessions / by-model tables | local transcripts of Claude Code, Codex, pi, oh-my-pi (omp), opencode |

Providers whose credentials aren't present are skipped silently. Nothing is
stored or sent anywhere else; the tool is read-only. See
[docs/how-it-works.md](docs/how-it-works.md) for the full data-source and
metric documentation.

## Build

```sh
cargo build --release        # binary at target/release/agent_usage
make install                 # install to ~/.local/bin/agent_usage
make install PREFIX=/usr/local   # or elsewhere (BINDIR = PREFIX/bin)
# alternatively: cargo install --path .   (installs to ~/.cargo/bin)
```

## Usage

```sh
./agent_usage                # everything: limits + today's sessions
./agent_usage --json         # normalized JSON for scripts / monitoring
./agent_usage --claude       # only Claude (limits + sessions)
./agent_usage --codex        # only Codex (limits + sessions)
./agent_usage --openrouter   # only OpenRouter (credits + pi/omp/opencode sessions)
./agent_usage --no-sessions  # skip the local transcript scan
AGENT_USAGE_DATE=2026-07-08 ./agent_usage   # session tables for a past day
```

Example output:

```
Claude (max)
  Session (5h)         ██░░░░░░░░░░░░░░░░░░  10%  resets 13:39 (in 3h 51m)
  Weekly (all models)  ████████░░░░░░░░░░░░  42%  resets Tue 14 Jul 04:00 (in 1d 18h)
  Weekly (Fable)       ████████████░░░░░░░░  59%  resets Tue 14 Jul 03:59 (in 1d 18h)

Codex (prolite)
  Session (5h)                  ██████████░░░░░░░░░░  48%  resets 14:06 (in 4h 18m)
  Weekly (all models)           ███████████░░░░░░░░░  53%  resets Fri 17 Jul 23:09 (in 5d 13h)
  5h (GPT-5.3-Codex-Spark)      ░░░░░░░░░░░░░░░░░░░░   0%  resets 14:48 (in 4h 59m)
  Weekly (GPT-5.3-Codex-Spark)  ░░░░░░░░░░░░░░░░░░░░   0%  resets Sun 19 Jul 09:48 (in 6d 23h)

OpenRouter (pay-as-you-go)
  Credits  ████░░░░░░░░░░░░░░░░  21%  $5.18 of $25.00
  Spend      today $0.00 · 7d $0.02 · 30d $0.02

Top sessions today
   1. claude moneymachine     fable-5   66 reqs  in  12.0k  out  37.1k  cache-r  26.0M  cache-w 424.5k  8c4045c2
   2. codex  polecat          gpt-5.5  124 reqs  in 246.9k  out  41.3k  cache-r  15.8M  cache-w      0  019f5715
   3. omp    local_llm        gpt-5.5    4 reqs  in  28.3k  out   1.3k  cache-r  67.6k  cache-w      0  $0.21  019f43a6
   ...

Today by model
  gpt-5.5  10 sessions  in   1.6M  out 220.7k  cache-r  50.5M  cache-w      0
  fable-5   2 sessions  in  12.1k  out 108.8k  cache-r  32.6M  cache-w 555.5k
```

Reset times are shown in local time with a relative countdown. Bars turn
yellow at 50% and red at 80%. `NO_COLOR` and non-TTY output disable color.
A `$` column appears for agents that record per-message cost (pi, omp,
opencode); subscription traffic has no per-message price.

Sessions are ranked by an estimated limit-consumption weight
(`input + 1.25·cache_write + 0.1·cache_read + 5·output`) — good for relative
ordering, not an absolute cost. Full details, including per-agent transcript
formats, deduplication rules, and midnight handling, are in
[docs/how-it-works.md](docs/how-it-works.md).

Exit code is 0 if at least one provider was fetched, 1 if all fail
(e.g. not logged in), 2 on bad arguments. If an OAuth token has expired,
the tool tells you to open the corresponding agent once to refresh it.

## JSON shape

```json
{
  "fetched_at": "2026-07-12T15:58:21Z",
  "providers": [
    {
      "provider": "claude",
      "plan": "max",
      "windows": [
        { "label": "Session (5h)", "used_percent": 6.0, "resets_at": "2026-07-12T20:40:00Z" }
      ]
    },
    {
      "provider": "openrouter",
      "plan": "pay-as-you-go",
      "windows": [
        { "label": "Credits", "used_percent": 20.7, "detail": "$5.18 of $25.00" },
        { "label": "Spend", "detail": "today $0.00 · 7d $0.02 · 30d $0.02" }
      ]
    }
  ],
  "errors": {},
  "sessions_today": [
    {
      "provider": "codex",
      "project": "polecat-1",
      "session_id": "019f5715-…",
      "model": "gpt-5.5",
      "requests": 99,
      "tokens": { "input": 235100, "output": 43900, "cache_read": 13200000, "cache_write": 0, "cost_usd": 0.0 }
    }
  ],
  "models_today": [
    { "model": "gpt-5.5", "sessions": 10, "tokens": { "…": 0 } }
  ]
}
```
