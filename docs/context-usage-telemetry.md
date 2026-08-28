# Context usage telemetry by agent harness

Investigation date: 2026-08-23. The runtime formats below were checked
against Claude Code 2.1.241, Codex CLI 0.148.0, OpenCode 1.17.9, the
current Pi source and documentation, and representative local session
artifacts. Runtime formats are not stable APIs unless the linked harness
documentation says otherwise, so collectors must tolerate missing and
unknown fields.

## Classification

- **Direct**: the harness or its model provider reports the value or
  lifecycle event explicitly.
- **Derived**: AMF must calculate the value from reported fields, append
  an estimate for unreported messages, or map a model ID to a catalog.
- **Unavailable**: the artifact has no usable field. A different runtime
  output may still make the value available.

"Current context" is deliberately separate from cumulative billing
usage. Repeated model calls resend much of the same context, so summing
turn usage can exceed the model window many times without describing the
tokens currently occupying it.

## Existing AMF records

AMF's `SessionTokenTracker` currently supports Claude, Codex, and
OpenCode, but not Pi. It refreshes cached usage entries no more often than
every 30 seconds even though session status sync is scheduled every five
seconds.

| Harness | Existing source | What AMF records | Suitability for current context |
| --- | --- | --- | --- |
| Claude Code | `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl` plus subagent transcripts | Sums every deduplicated assistant response's `input_tokens`, `output_tokens`, `cache_read_input_tokens`, and `cache_creation_input_tokens`. | Cumulative billing usage only. The latest response is useful, but the existing aggregate is not. |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl`, located through `~/.codex/state_5.sqlite` when possible | Keeps `total_token_usage` (or accumulates `last_token_usage` when totals are absent). | `total_token_usage` is cumulative. The existing parser discards the live `last_token_usage` and `model_context_window` fields needed here. |
| OpenCode | Legacy `~/.local/share/opencode/storage/{session,message,part}` JSON | Sums every `step-finish` part's input, output, reasoning, cache-read, and cache-write tokens. | Cumulative billing usage only. OpenCode 1.17.9 uses `~/.local/share/opencode/opencode.db`; AMF's existing token reader does not read that SQLite store. |
| Pi | None | `provider_for_session_kind()` returns no provider for `SessionKind::Pi`. | Unavailable until a Pi collector is added. |

This feature therefore needs a context-specific sample path. It must not
reuse `SessionTokenUsage.total_tokens` as window occupancy.

## Field matrix

The table classifies the best available value for each requested field.
Details and fallback rules follow it.

| Harness | Input tokens | Cached tokens | Context limit | Conversation identity | Compaction / summary | Fresh session |
| --- | --- | --- | --- | --- | --- | --- |
| Claude Code | **Direct**: status-line `context_window.current_usage.input_tokens`; also latest transcript assistant `message.usage.input_tokens` | **Direct**: `cache_read_input_tokens` and `cache_creation_input_tokens` | **Direct**: status-line `context_window.context_window_size`; **derived** only if AMF falls back to a model catalog | **Direct**: `session_id` in status-line, hook, and transcript data | **Direct**: `PreCompact`, then `SessionStart.source == "compact"`; current usage becomes null until the next response | **Direct**: `SessionStart.source == "startup"` or `"clear"`; changed `session_id` is additional evidence |
| Codex | **Direct**: rollout `event_msg/token_count.info.last_token_usage.input_tokens` | **Direct**: `cached_input_tokens` and `cache_write_input_tokens` (cached input is a subset of input) | **Direct**: `token_count.info.model_context_window` | **Direct**: rollout `session_meta.payload.id` / `session_id`; also the thread ID in `state_5.sqlite` | **Direct**: rollout `type == "compacted"` and `event_msg.payload.type == "context_compacted"` | **Direct**: a new rollout `session_meta` with a new thread ID |
| OpenCode | **Direct**: latest successful, non-summary assistant `tokens.input` or `step-finish.tokens.input` | **Direct**: `tokens.cache.read` and `tokens.cache.write` | **Direct at runtime**: selected `Provider.Model.limit.context`; **derived** when AMF resolves stored provider/model IDs through a catalog; **unavailable** if that lookup fails or returns zero | **Direct**: `session.id` / SQLite `session.id` | **Direct**: v1 compaction user part plus a completed assistant with `summary: true`; v2 uses durable compaction-started/ended events | **Direct**: a newly created `session.id`; `parent_id` distinguishes forks |
| Pi | **Direct**: assistant-message `usage.input` | **Direct**: `usage.cacheRead` and `usage.cacheWrite` | **Direct at runtime**: active model `contextWindow`, also returned as `get_session_stats.contextUsage.contextWindow`; **derived** when resolving the persisted provider/model through Pi's model registry | **Direct**: session header `id` | **Direct**: JSONL entry `type == "compaction"` with `tokensBefore`; `session_compact` is the live extension event | **Direct**: a new JSONL session header ID; `parentSession` identifies a fork/clone |

## Collector conclusions

### Claude Code

Claude Code's status-line command receives the best direct snapshot:

- `context_window.current_usage` contains the most recent API response's
  input, output, cache-creation, and cache-read counts.
- `context_window.context_window_size` is the effective window, including
  extended-context selection.
- `context_window.used_percentage` is already calculated by Claude Code.
  Its numerator is input-only:
  `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
- `current_usage` is null before the first response and immediately after
  compaction. That is an unknown/reset sample, not zero-token direct
  telemetry.

The latest assistant transcript record exposes the same input components,
so AMF can derive the numerator when no status-line sidecar is available.
The transcript does not expose the effective context-window size, making a
model/config lookup an estimated fallback. The existing cumulative AMF
totals must never be used for the numerator.

Claude's hook lifecycle provides reliable reset evidence. `PreCompact`
fires before manual or automatic compaction. `SessionStart` reports
`source` as `startup`, `resume`, `clear`, or `compact`; `clear` and
`compact` reset the context indicator, while `resume` does not. Both hooks
include `session_id` and `transcript_path`.

Relevant upstream references:

- [Claude Code status-line schema and context formula](https://code.claude.com/docs/en/statusline)
- [Claude Code hook inputs and SessionStart sources](https://code.claude.com/docs/en/hooks)
- [Claude Code session transcript location and `/clear`/`/compact` behavior](https://code.claude.com/docs/en/sessions)

### Codex

Each rollout `token_count` event contains:

- `info.last_token_usage`: the most recent/current request breakdown;
- `info.total_token_usage`: cumulative thread billing usage; and
- `info.model_context_window`: the effective context limit.

The direct recorded occupancy is `last_token_usage.total_tokens`, matching
Codex's own context manager. `last_token_usage.input_tokens` is the direct
input count. `cached_input_tokens` is included within input and must not be
added to it a second time. Events can briefly contain a synthetic
post-compaction total before the next provider response, but it is still
Codex's current context accounting and is paired with an explicit reset
event.

Compaction is durably explicit: current rollouts can contain a top-level
`compacted` record and an `event_msg` whose payload type is
`context_compacted`. A new conversation starts a new rollout with a
`session_meta` ID. A changed ID is authoritative even if counters happen to
look continuous.

Relevant upstream references:

- [Codex thread token-usage notification schema](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/schema/json/v2/ThreadTokenUsageUpdatedNotification.json)
- [Codex context manager's use of last-turn tokens](https://github.com/openai/codex/blob/main/codex-rs/core/src/context_manager/history.rs)
- [Codex model context-window configuration](https://github.com/openai/codex/blob/main/codex-rs/config/src/config_toml.rs)

### OpenCode

OpenCode's assistant/`step-finish` token object is per model call, not a
session total. For the latest successful non-summary assistant response:

- prefer `tokens.total` when it is present and nonzero;
- otherwise derive occupancy as
  `input + output + cache.read + cache.write`, which is OpenCode 1.17.9's
  own overflow formula;
- do not sum all steps when calculating the current window.

The assistant record also carries `providerID` and `modelID`. The selected
model registry carries `limit.context`; old and current stored message rows
do not persist that limit, so an offline collector must resolve the model
and classify that result as derived. Unknown/zero limits produce no
percentage.

For v1 sessions, a compaction request is a user message with a
`type: "compaction"` part, and completion is a finished, non-error assistant
message with `summary: true`. `session.time_compacting` is only transient
state. OpenCode's developing v2 format instead persists explicit
compaction-started and compaction-ended events; the collector should accept
both formats. New sessions have new session IDs; a parent ID indicates a
fork rather than continued occupancy.

OpenCode moved from the legacy JSON storage tree to
`~/.local/share/opencode/opencode.db`. Supporting only AMF's current JSON
reader would silently miss current releases, so the collector must support
SQLite and retain legacy JSON compatibility.

Relevant upstream references:

- [OpenCode 1.17.9 assistant token schema](https://github.com/anomalyco/opencode/blob/v1.17.9/packages/core/src/v1/session.ts)
- [OpenCode 1.17.9 overflow calculation](https://github.com/anomalyco/opencode/blob/v1.17.9/packages/opencode/src/session/overflow.ts)
- [OpenCode 1.17.9 compaction lifecycle](https://github.com/anomalyco/opencode/blob/v1.17.9/packages/opencode/src/session/compaction.ts)
- [OpenCode provider model limits](https://github.com/anomalyco/opencode/blob/v1.17.9/packages/opencode/src/provider/provider.ts)

### Pi

Pi persists sessions as tree-shaped JSONL under
`~/.pi/agent/sessions/<encoded-cwd>/`. Assistant messages carry provider,
model, and a direct `usage` object with input, output, cache-read,
cache-write, and `totalTokens`. Session headers carry the conversation ID,
working directory, and optional parent session.

Pi already defines the desired normalized runtime value. The
`get_session_stats` RPC response exposes `contextUsage.tokens`,
`contextWindow`, and `percent`; extension code can obtain the same value
from `ctx.getContextUsage()`. Pi uses the latest valid assistant usage and
estimates tokens for trailing messages. Consequently:

- a snapshot based only on the latest provider `usage.totalTokens` is
  direct;
- a snapshot with estimated trailing messages, or one estimated wholly
  from messages because provider usage is absent, is estimated;
- after compaction, tokens and percent are null until a valid
  post-compaction assistant response exists.

AMF launches Pi in interactive rather than RPC mode, so it cannot send
`get_session_stats` to the existing pane. A file collector can reproduce
Pi's documented calculation from JSONL and the model registry; a local Pi
extension sidecar could expose the normalized runtime value without
changing the global Pi configuration.

Pi compaction is explicit in the transcript as a `compaction` entry with
`tokensBefore`, `firstKeptEntryId`, and a summary. A fresh conversation or
fork writes a new session header and ID, with `parentSession` populated for
the latter.

Relevant upstream references:

- [Pi session file format](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/session-format.md)
- [Pi compaction format and thresholds](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/compaction.md)
- [Pi RPC session statistics](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/rpc.md)
- [Pi context-usage implementation](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/core/agent-session.ts)
- [Pi model context-window configuration](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/models.md)

## Reset signals to carry into implementation

Use explicit lifecycle evidence before counter heuristics:

1. A changed conversation/session/thread ID is an authoritative fresh
   conversation.
2. Claude `SessionStart(clear|compact)`, Codex `compacted` or
   `context_compacted`, OpenCode completed compaction, and Pi `compaction`
   entries are authoritative resets.
3. A post-reset unknown/null sample stays unknown until the harness emits a
   valid new measurement; it must not be displayed as a fresh `0%`.
4. Token rollback is only a fallback for formats where explicit events were
   missed. Minor provider corrections are not reset evidence by themselves.

The exact rollback tolerance, stale age, and persisted snapshot policy
belong to later plan tasks.
