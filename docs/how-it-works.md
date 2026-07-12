# How agent_usage works

`agent_usage` has two independent data paths that run concurrently:

1. **Limits** — live HTTP calls to each provider's own usage API, authenticated
   with the OAuth tokens / API keys the agents already store on disk.
2. **Sessions** — a scan of today's local session transcripts from every
   supported agent, aggregated into "Top sessions today" and "Today by model".

Nothing is written anywhere; credentials never leave the machine except to the
provider that issued them.

## 1. Limits (network)

| Provider | Credentials read | Endpoint |
|---|---|---|
| Claude Code | `~/.claude/.credentials.json` → `claudeAiOauth.accessToken` | `GET https://api.anthropic.com/api/oauth/usage` with `anthropic-beta: oauth-2025-04-20` |
| Codex | `~/.codex/auth.json` → `tokens.access_token`, `tokens.account_id` | `GET https://chatgpt.com/backend-api/codex/usage` with `chatgpt-account-id` header |
| OpenRouter | first key found in `~/.pi/agent/auth.json` or `~/.local/share/opencode/auth.json` (`openrouter.key`) | `GET https://openrouter.ai/api/v1/credits` and `/api/v1/key` |

These are the same endpoints the agents' own `/usage` and `/status` screens
call. A provider is only queried if its credential file exists; each fetch
runs on its own thread while the main thread does the session scan.

- **Claude** returns a `limits` array (session 5h, weekly all-models, weekly
  model-scoped e.g. "Fable") with `percent` and `resets_at`. We prefer that
  array and fall back to the older `five_hour`/`seven_day` shape. Extra-usage
  credits appear as another row when enabled on the account.
- **Codex** returns `rate_limit.primary_window` (5h) and `secondary_window`
  (weekly) as `used_percent` + `reset_at`, plus `additional_rate_limits` for
  model-scoped meters (e.g. GPT-5.3-Codex-Spark). Window labels are derived
  from `limit_window_seconds` (18000 → "5h", 604800 → "Weekly").
- **OpenRouter** has no rate windows; we show credits consumed
  (`total_usage / total_credits`), the key spend limit if one is set, and
  daily/weekly/monthly spend.

Expired tokens are detected before the call (Claude stores `expiresAt`) or
mapped from a 401, and reported as "open `claude`/`codex` once to refresh" —
this tool never refreshes or mutates tokens itself.

## 2. Sessions (local files, no network)

All scanners follow the same rules:

- **File selection**: only files whose mtime falls on the target day are
  opened (a session started yesterday but active today still qualifies).
- **Line filtering**: within a file, each usage record's own timestamp must
  fall on the target day (local timezone), so sessions spanning midnight only
  count today's part.
- The target day is today, or `AGENT_USAGE_DATE=YYYY-MM-DD` to inspect a past
  day.
- Sessions whose ranking weight is zero (e.g. free/local models reporting no
  tokens) are dropped.

Per-agent specifics:

| Agent | Location | Format | Notes |
|---|---|---|---|
| Claude Code | `~/.claude/projects/<proj>/<session>.jsonl` | one JSON line per event; assistant lines carry `message.usage` | Streaming rewrites the same message across lines → deduplicated by `message.id`, so one API response counts once. `<synthetic>` (error) messages skipped. Project name from the transcript's `cwd`. |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | `event_msg`/`token_count` events | We sum per-request deltas (`last_token_usage`) instead of the cumulative `total_token_usage`, which would double-count resumed sessions. Model from `turn_context` (the meta line's model is often null). `input` includes cached tokens, so cache reads are split out via `cached_input_tokens`. |
| pi | `~/.pi/agent/sessions/<proj>/<ts>_<id>.jsonl` | `session` header + `message` lines | Assistant messages carry `usage {input, output, cacheRead, cacheWrite, cost.total}`. Written once per response — no dedup needed. |
| oh-my-pi (omp) | `~/.omp/agent/sessions/…` | same as pi | Same parser. omp also snapshots Codex rate limits into `agent.db`'s `usage_history` table, but those cover the same ChatGPT account the Codex provider already reports live. |
| opencode | `~/.local/share/opencode/storage/message/<sessionID>/msg_*.json` | one JSON file per message | Assistant files carry `tokens {input, output, reasoning, cache{read,write}}` and `cost` (USD). `reasoning` is billed as output, so it's added to `output`. Project from `path.cwd`. Session dirs untouched today are skipped via dir mtime. |

## 3. Metric definitions

For every session and model row:

- **in** — non-cached input tokens. (Codex reports cached as a subset of
  input; we subtract it. Claude/pi/opencode already report them separately.)
- **out** — output tokens, including reasoning tokens where the agent splits
  them out (opencode).
- **cache-r / cache-w** — prompt-cache reads and writes. Only Anthropic bills
  cache writes distinctly; other providers show 0.
- **reqs** — number of API responses (per-message for Claude/pi/omp/opencode,
  per `token_count` event for Codex).
- **$** — summed per-message cost as recorded by the agent itself (pi, omp,
  opencode). Subscription traffic (Claude, Codex) has no per-message price and
  shows no cost. Local models cost $0.00 and are omitted.

### Ranking weight

Sessions and models are ordered by an estimated limit-consumption weight:

```
weight = input + 1.25 × cache_write + 0.1 × cache_read + 5 × output
```

The coefficients mirror typical provider pricing ratios (cache reads cost
~10% of input, cache writes ~125%, output ~5x). This keeps a session that
generated 40k output tokens ahead of one that merely re-read 13M cached
tokens. It is a **relative ordering heuristic**, not a dollar amount and not
the providers' actual (undisclosed) limit accounting.

## Caveats

- opencode can also authenticate to OpenAI via ChatGPT OAuth; that traffic
  draws from the same Codex subscription limits shown in the Codex block, but
  its sessions are tagged `opencode`, not `codex`.
- pi and opencode share the same OpenRouter key on this machine; the
  OpenRouter block reports the key's account-level totals once.
- Provider usage APIs are undocumented and may change shape; parsers are
  written with optional fields throughout so missing keys degrade to zeros
  rather than errors.
