# usage

Print coding-agent subscription usage in the terminal — no web UI needed.

Reads the credentials your locally installed agents already store on disk,
queries each provider's own usage API for **rate limits**, and scans local
session transcripts for **today's per-session / per-model usage**:

| Provider block | Source |
|---|---|
| Claude (session/weekly limits) | `~/.claude/.credentials.json` → `api.anthropic.com/api/oauth/usage` |
| Codex (session/weekly limits) | `~/.codex/auth.json` → `chatgpt.com/backend-api/codex/usage` |
| Cursor (included-usage limits) | `~/.config/cursor/auth.json` → `api2.cursor.sh` DashboardService |
| OpenRouter (credits & spend) | key from pi/opencode → `openrouter.ai/api/v1/{credits,key}` |
| Top sessions / by-model tables | local transcripts of Claude Code, Codex, pi, oh-my-pi (omp), opencode |

Providers whose credentials aren't present are skipped silently. Nothing is
stored or sent anywhere else; the tool is read-only and each credential is
only ever sent to the provider that issued it. See
[docs/how-it-works.md](docs/how-it-works.md) for the full data-source and
metric documentation.

> **Disclaimer**: this project is not affiliated with Anthropic, OpenAI, or
> OpenRouter. The Claude and Codex usage endpoints are undocumented internal
> APIs used by the agents' own status screens — they may change or disappear
> without notice, and your use of them is subject to each provider's terms of
> service. Transcript formats are similarly unstable; parsers degrade to
> zeros rather than erroring when fields go missing.

**Platform support**: Linux and macOS. Windows is untested and unsupported
(agent config paths and `HOME` resolution differ).

## Build

```sh
cargo build --release        # binary at target/release/usage
make install                 # install to ~/.local/bin/usage
make install PREFIX=/usr/local   # or elsewhere (BINDIR = PREFIX/bin)
# alternatively: cargo install --path .   (installs to ~/.cargo/bin)
```

## Usage

```sh
./usage                # everything: limits + today's sessions
./usage --json         # normalized JSON for scripts / monitoring
./usage --claude       # only Claude (limits + sessions)
./usage --codex        # only Codex (limits + sessions)
./usage --cursor       # only Cursor (included-usage limits)
./usage --openrouter   # only OpenRouter (credits + pi/omp/opencode sessions)
./usage --no-sessions  # skip the local transcript scan
./usage --history 30   # widen the daily graph window (default 7, max 90)
./usage --no-history   # skip the daily usage graph
AGENT_USAGE_DATE=2026-07-08 ./usage   # session tables for a past day
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

Cursor (Pro)
  Included usage (month)  ██░░░░░░░░░░░░░░░░░░  10%  $2.00 of $20.00  resets Sat 18 Jul 09:42 (in 4d 21h)

OpenRouter (pay-as-you-go)
  Credits  ████░░░░░░░░░░░░░░░░  21%  $5.18 of $25.00
  Spend      today $0.00 · 7d $0.02 · 30d $0.02

Top sessions today
┌───┬────────┬──────────────┬─────────┬──────┬────────┬────────┬─────────┬─────────┬───────┬──────────┐
│ # │ agent  │ project      │ model   │ reqs │     in │    out │ cache-r │ cache-w │  cost │ session  │
├───┼────────┼──────────────┼─────────┼──────┼────────┼────────┼─────────┼─────────┼───────┼──────────┤
│ 1 │ claude │ moneymachine │ fable-5 │   66 │  12.0k │  37.1k │   26.0M │  424.5k │       │ 8c4045c2 │
│ 2 │ codex  │ polecat      │ gpt-5.5 │  124 │ 246.9k │  41.3k │   15.8M │       0 │       │ 019f5715 │
│ 3 │ omp    │ local_llm    │ gpt-5.5 │    4 │  28.3k │   1.3k │   67.6k │       0 │ $0.21 │ 019f43a6 │
└───┴────────┴──────────────┴─────────┴──────┴────────┴────────┴─────────┴─────────┴───────┴──────────┘

Today by model
┌─────────┬──────────┬────────┬────────┬─────────┬─────────┐
│ model   │ sessions │     in │    out │ cache-r │ cache-w │
├─────────┼──────────┼────────┼────────┼─────────┼─────────┤
│ gpt-5.5 │       10 │   1.6M │ 220.7k │   50.5M │       0 │
│ fable-5 │        2 │  12.1k │ 108.8k │   32.6M │  555.5k │
└─────────┴──────────┴────────┴────────┴─────────┴─────────┘
```

The `cost` column only appears when at least one row has a recorded
per-message cost (pi, omp, opencode).

A trailing daily-usage graph is shown by default (7 days; `--history N` for
up to 90, `--no-history` to skip), built from the same local transcripts,
with one stacked bar per day colored by agent:

```
Daily usage — last 7 days (weighted tokens)
  Wed Jul 08  █████▓▓                                   16.5M
  Thu Jul 09  █████▓                                    20.7M
  Fri Jul 10  ████████▓▓                                42.7M
  Sat Jul 11  ██▓                                        7.4M
  Sun Jul 12  ███████████▓▓▓▓▓▓▓▓▓▓▓▓                  128.4M
  Mon Jul 13  ████████████████▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ 178.1M
  Tue Jul 14  ██▓▓                                      21.6M
  █ claude  ▓ codex
```

Cursor is absent from the graph for the same reason it has no session rows:
no local token data. Historical limit-percentages aren't shown because
providers only report the current window — daily consumption is the
available signal.

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

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT) at your option.
