# Project config location

- **Status:** Complete. `amf.json` is the config path; `.amf/config.json`
  is still read and migrates on the next write.
- **Owner:** unassigned
- **Relates to:** `src/extension.rs`
  (`merge_project_extension_config`, `save_project_extension_config`),
  `src/app/config_wizard.rs`, `src/app/prompt_library.rs`, `.gitignore`

## Why / problem

`.amf/` inside a repo holds two kinds of file that want opposite
treatment:

- **Generated runtime artifacts** — `session-status/`,
  `opencode-sidebar/`, `review-memory.md`. Never part of a branch's real
  changes, so `.gitignore` ignores `.amf/` dir-wide.
- **`config.json`** — hand-authored shared project config (custom
  sessions, feature presets, lifecycle hooks, keybindings, plan
  questions, prompt templates). It is meant to be committed.

The two are reconciled today by **force-tracking one file inside an
ignored directory**:

```gitignore
# AMF / opencode runtime-generated artifacts — never part of a branch's
# real changes. (.amf/config.json is intentional shared config and is
# force-tracked despite this dir-wide ignore.)
.amf/
```

That exception costs something every time:

- Committing config needs `git add -f`. A plain `git add .amf/config.json`
  fails with an "ignored by one of your .gitignore files" error, which
  reads like a mistake rather than the documented design.
- "Is this file generated?" can't be answered from the ignore rules
  alone — the rules say the whole directory is, and only a comment says
  otherwise.
- Any new generated artifact dropped into `.amf/` is safe, but any new
  *config* file there silently isn't tracked, with no error at all.

## Proposed design

Move the hand-authored config **out** of `.amf/`, leaving that directory
purely generated and honestly ignorable.

```text
repo/
├── amf.json          ← tracked, hand-authored, committed normally
├── .amf/             ← generated only, gitignored with no exceptions
│   ├── README.md     ← injected: says the dir is generated, points at amf.json
│   ├── session-status/
│   └── opencode-sidebar/
├── Cargo.toml
└── src/
```

`amf.json` sits at the repo root, visible, alongside `Cargo.toml` and
`package.json` — the same place a reader already looks for project
configuration. Visible file = tracked and yours; hidden directory =
generated and ours.

### Path resolution

One resolver, no path literals scattered across call sites:

- `project_config_path(dir)` → `{dir}/amf.json`. **All writes** go here.
- `legacy_project_config_path(dir)` → `{dir}/.amf/config.json`.
- `resolve_project_config_path(dir)` → prefers `amf.json`, falls back to
  the legacy path when only that exists, `None` when neither does.

The same pair applies to worktree-scoped config (`{workdir}/amf.json`),
which the prompt library exports to.

### Migration

Reads keep working against existing checkouts with no user action:
`resolve_project_config_path` finds the legacy file. The first **write**
migrates — `amf.json` is written, and the legacy file is removed only
after that write succeeds, so a crash mid-migration leaves the old file
intact rather than losing config. Because reads prefer the new path,
leaving both behind would make the stale one invisibly authoritative-
looking; removing it is what keeps one answer to "where does config
live".

### The README

`.amf/` is created lazily by whichever subsystem needs it first
(session status, opencode sidecar). A single `ensure_generated_amf_dir`
helper creates the directory and drops a `README.md` explaining that
everything inside is generated, that it is safe to delete, and that
hand-authored config belongs in `amf.json` at the repo root. Anyone who
opens the directory wondering what it is gets the answer in place.

## Progress

- [x] Path helpers + `resolve_project_config_path` fallback in
      `src/extension.rs`
- [x] Update read sites: `merge_project_extension_config`,
      `load_project_extension_scope`, `load_project_prompt_templates`,
      `template_source_path`
- [x] Update write sites: `save_project_extension_config`,
      `export_template_to_project_config`, `remove_template_from_config`
- [x] Legacy removal after successful migrating write
- [x] `generated_amf_subdir` + injected `.amf/README.md`, wired into the
      `.amf/` creation sites
- [x] Drop the force-track exception from `.gitignore`; migrate AMF's own
      `.amf/config.json` → `amf.json`
- [x] Tests: fallback read, migrating write, no-legacy case, README
      injection
- [x] CHANGELOG entry + CLAUDE.md / docs references

## Open questions

- Should `amf doctor` report a repo still on the legacy path, so the
  migration is visible before someone hits it?
- Worktree-scoped `amf.json` is untracked in practice (worktrees are
  ignored). Worth a distinct name, or is same-name-different-scope
  clearer?
