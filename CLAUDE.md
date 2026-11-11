# CLAUDE.md

@AGENTS.md

Everything above is shared with Codex, OpenCode and Pi. The rest of this file
is specific to Claude Code working in this repository.

## Keep this file small

This file and `AGENTS.md` are loaded into every session and most subagents,
not `Explore` or `verify`, which set `omitClaudeMd: true`. Put
feature-specific detail in `docs/development/features/` and link it from the
table in `AGENTS.md`; add here only what every task needs.

## Model selection

Lineup checked 2026-11-11. Prices are per million input/output tokens,
Anthropic API:

| Alias | Model | Price | Use for |
| --- | --- | --- | --- |
| `fable` | Fable 5.2 | $8 / $40 | Hardest, longest-running work |
| `opus` | Opus 5.5 | $4 / $20 | Design, judgment, unknown-cause debugging |
| `sonnet` | Sonnet 5.5 | $2 / $10 | Well-specified implementation |
| `haiku` | Haiku 5.5 | $0.10 / $0.50 | Search, lookups, log reading (5× above 100K prompt tokens) |

- **Keep the session on one model.** The prompt cache is per model, so
  switching a long session re-reads the whole context uncached. Save money
  with subagents instead: each starts with a fresh, small context.
- **Always pass `model` on an Agent call.** An omitted `model` inherits the
  session's, which may be Fable. Pick the cheapest tier that fits:
   - `haiku`: locating code, grep sweeps, running checks and reading
     CI/test/debug-log output, summarizing a file or diff.
   - `sonnet`: implementing from a concrete plan, writing tests for a stated
     behavior, doc updates, a focused review of a small diff.
   - `opus`: design, debugging with an unknown cause, and any change touching
     migrations, the shared `amf.db`, process/child lifetimes, worker identity
     checks, or an invariant in a feature note.
   - `fable`: only when the user asks.
- **Lower effort before a smaller model.** For routine steps, passing a lower
  `effort` to a capable model often holds quality better than dropping a tier.
- **Suggest Fable, don't switch to it.** For a cross-module refactor or a
  concurrency/lifetime bug Opus has failed on twice, tell the user
  `/model fable` may be worth the 2× cost.

### When to delegate

A subagent starts cold and must be told everything, so it only saves tokens
when its work would otherwise fill the session's context.

- **Delegate** when only the conclusion matters and the path to it is noisy
  (searches across many files, test and CI output), or when the work is
  self-contained and can be fully specified in a short brief.
- **Do it inline** when the change is a few edits in files already read, when
  it depends on what was discussed in the session, or when writing the brief
  costs about as much as doing the work.

Project agents in `.claude/agents/`:

| Agent | Model | Use for |
| --- | --- | --- |
| `Explore` | haiku | Finding where code lives; returns `path:line`, not file dumps |
| `verify` | haiku | Running build, test, fmt and Clippy; returns only failures |
| `implementer` | sonnet | Carrying out a concrete, already-decided change |

### Orchestrating (session on Fable or Opus)

For multi-step work the session plans, decides and reviews; subagents read
and write.

1. Locate with `Explore`. Read only the excerpts needed to decide.
2. Decide the design in the session. Send a hard sub-question to an `opus`
   subagent only if it can be answered without the session's context.
3. Hand each independent piece to an `implementer`, in one message so they
   run in parallel. Give two agents the same file only with
   `isolation: "worktree"`.
4. Check with `verify`, then review the diff in the session, not the
   implementer's summary of it.

A brief names the files, the decision already made, the acceptance criteria,
the focused test command, and what to return (a few lines, not a transcript).
Continue a running agent with SendMessage instead of spawning a replacement.

### Without an orchestrator (session on Sonnet or Haiku, or a small task)

Do the work directly. Still send broad searches to `Explore` and long check
runs to `verify`, because their output is what fills the context. When a
design question or an unknown-cause bug comes up, ask an `opus` subagent that
one question instead of switching the session model.

### Wired in

`/amf:pr-status` forks onto Haiku. `/build`, `/check`, `/lint`,
`/amf:pr-create` and `/amf:pr-continue` inherit the session model on purpose:
they depend on the session's context. AMF's product-side model pickers and
advice (`model_evidence.rs`, `model_analysis/`) are separate from this section.
