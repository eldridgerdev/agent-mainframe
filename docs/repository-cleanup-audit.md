# Repository Cleanup Audit

## Purpose and scope

This report audits the tracked repository for cleanup opportunities without removing or consolidating content. It covers Rust source and manifests, documentation, automation, examples, Docker assets, bundled runtime content, plugins and skills, themes, GitHub configuration, and tracked tool-specific configuration. Findings distinguish confirmed candidates from items that require runtime or maintainer confirmation.

## Finding format and rating rubric

Each finding uses the following fields:

- **Location / scope:** Tracked paths and workflows affected by the finding.
- **Category:** Safe cleanup, consolidation, documentation correction, or needs confirmation.
- **Evidence:** Static references, build or packaging paths, documented workflows, runtime discovery, and relevant git history.
- **Recommendation:** The proposed follow-up; no cleanup is performed by this audit.
- **Risk:** Impact if the recommendation is wrong: low, medium, or high.
- **Confidence:** Strength of the evidence: high (direct and corroborated), medium (strong static evidence with an unresolved dynamic path), or low (incomplete external/runtime evidence).
- **Verification status:** Confirmed, partially verified, or unverified, followed by the checks still needed.
- **Expected impact:** The maintenance benefit and approximate scope of a future change.

## Executive summary

The audit found 12 actionable items: three safe cleanups, three documentation corrections, three consolidation opportunities, and three candidates requiring maintainer or runtime confirmation. The highest-confidence cleanup items are a broken OpenCode settings symlink, an unused development dependency, and a malformed CI command pipeline. Two additional runtime/configuration gaps deserve prompt follow-up: a Status Ticker entry points to a script that has never been tracked, and OpenCode cleanup omits the injected Gruvbox theme.

Most superficially suspicious bundled content is active. Release/Docker scripts, hook scripts, OpenCode plugins, skills, canonical themes, syntax fixtures, and the retained custom diff-review hook all have compile-time, runtime, test, packaging, or documented consumers. The report therefore recommends retaining them. No Rust behavior, configuration, or repository content was cleaned up as part of this feature; this Markdown report is the only tracked artifact added.

## Methodology

1. Captured the tracked-file inventory at `HEAD` and grouped every file by primary purpose.
2. Traced Rust modules, Cargo targets/dependencies, `include_str!` assets, scripts, symlinks, workflow calls, runtime injection, and cleanup paths.
3. Ran `cargo check`, `cargo build`, `cargo check --all-targets`, `cargo test`, and `cargo clippy`; parsed tracked JSON and checked shell syntax.
4. Reviewed repository documentation and checked every relative Markdown link for an existing target.
5. Compared `AGENTS.md` and `CLAUDE.md` section by section against current source signatures and structure.
6. Inspected full path history for every proposed unused/obsolete item and used `v0.24.0..HEAD` as the recent-release window.
7. Cross-checked findings against install/upgrade, release packaging, automation, theme injection, syntax parser, worktree, and agent-session workflows.

Direct compile-time inclusion, a current runtime caller/discovery path, a release/CI packaging path, or a documented workflow corroborated by tests counts as evidence of use. Absence of a static reference alone does not justify removing a standalone tool; such cases remain “needs confirmation.”

## Repository inventory and validation baseline

Inventory captured from `git ls-files` at commit `14a3938` (2026-06-23): 229 tracked files.

| Group | Paths | Files |
|---|---|---:|
| Rust source | `src/` | 121 |
| Cargo manifests | `Cargo.toml`, `Cargo.lock` | 2 |
| Documentation | root Markdown/text files and `docs/` | 44 |
| Automation | `scripts/`, `.github/` | 21 |
| Examples | `examples/` | 1 |
| Docker assets | `docker/` | 3 |
| Bundled runtime content | `plugins/`, `skills/`, `themes/` | 13 |
| Tool-specific configuration | `.amf/`, `.claude/`, `.opencode/`, `.gitignore` | 24 |

The inventory deliberately counts files by primary purpose. For example, skill Markdown is classified as bundled runtime content, GitHub workflows as automation, and syntax fixtures under `docs/` as documentation even when they also exercise runtime behavior.

Validation baseline captured on 2026-08-10 with `rustc 1.96.1` and `cargo 1.96.1`:

| Command | Result | Baseline notes |
|---|---|---|
| `cargo check` | Pass | Completed without warnings or errors. |
| `cargo clippy` | Pass with warnings | Generated 94 warnings and no errors. The warnings are pre-existing maintenance debt; representative groups include collapsible conditionals, excessive argument counts, large enum variants, unnecessary borrows/conversions, and duplicated conditional branches. |
| `cargo build` | Pass | Debug binary built successfully without compiler warnings. |
| `cargo check --all-targets` | Pass | Confirmed the binary, unit-test configurations, and auto-discovered `examples/vtcheck.rs` target compile. |
| `cargo test` | Pass | 615 tests passed; 0 failed, ignored, or filtered. This directly contradicts the “There are no tests yet” statement shared by `AGENTS.md` and `CLAUDE.md`. |

`cargo build --release` was not run because the audit does not change compiled or packaged behavior and the debug build exercises the same source graph. Runtime checks requiring an interactive tmux/agent environment are addressed as retained verification needs rather than baseline build checks.

## Declaration and consumer trace

- `Cargo.toml` declares one binary (`amf`), one auto-discovered example (`vtcheck`), no Cargo features, and no build script. `cargo check --all-targets` resolves both targets.
- Every tracked file under `src/` is connected through the module declarations rooted at `src/main.rs`; nested module roots in `src/app/mod.rs`, `src/handlers/mod.rs`, `src/ui/mod.rs`, `src/db/mod.rs`, and `src/highlight/mod.rs` account for the directory modules and test-only modules.
- All 25 normal dependencies have direct source references. `mockall` is used by test configurations; `pretty_assertions` has no source or example reference and is an apparent unused development dependency pending history review.
- Runtime-injected scripts, OpenCode plugins, skills, and themes are connected through `include_str!` declarations in `src/app/setup.rs` and `src/theme.rs`. The syntax fixtures used at compile time are connected through `include_str!` in syntax/highlight tests.
- Release, Docker, automation, and PR helper scripts have workflow or documentation consumers. Three scripts have no tracked caller: `scripts/generate-amf-themes.sh`, `scripts/generate-amf-themes.js`, and `scripts/test-thinking.sh`. Their comments and behavior identify them as standalone maintenance/development utilities, so absence of a static caller is not by itself removal evidence.

## Documentation review notes

- All relative Markdown links in the tracked documentation resolve to an existing repository path.
- The [README prompt-library section](../README.md#prompt-library) says fill-in placeholders and declarative templates are future work. Both are implemented and documented in `CHANGELOG.md` (`v0.26.0` and Unreleased), `src/prompt_library.rs`, and `src/app/prompt_library.rs`.
- The [README bundled-theme list](../README.md#bundled-opencode-themes), `themes/README.md`, `themes/opencode/README.md`, and `.opencode/themes/README.md` disagree with the four files embedded by `src/theme.rs`: they omit Gruvbox in one or more directory listings, alternate between transparent and stable non-transparent background claims, and identify an obsolete injection location (`src/app/mod.rs` instead of `src/app/setup.rs`). Only `themes/opencode/README.md` adds durable maintainer value by documenting how embedded themes are updated; `themes/README.md` is a redundant directory overview and `.opencode/themes/README.md` is a stale duplicate aimed at generated/project-local copies. Remove those two redundant READMEs and correct the retained maintainer guide.
- `docs/syntax-tests/README.md` lists only three fixtures although 20 fixture files are tracked in that directory. Several are compile-time inputs to highlighter/diff tests, while others are manual fixtures; the README does not distinguish the two groups.
- `docs/backlog/README.md` says the bug backlog currently tracks the dashboard `A` bug, but the only entry in `docs/backlog/bug-backlog-plan.md` is marked fixed. This is stale index text rather than evidence that the backlog mechanism itself is obsolete.
- The `README.md` Architecture Notes section still describes a fixed 50 ms/250 ms polling scheme. It duplicates the preceding How It Works material and the detailed `AGENTS.md`/`CLAUDE.md` guidance while creating another synchronization surface. Remove the section and keep the existing pointer to the agent guidance files.
- Historical statements in `CHANGELOG.md` were treated as release records rather than rewritten to match current behavior. One apparent contradiction—the `v0.26.0` removal of bundled legacy diff-review scripts while `plugins/diff-review/scripts/custom-diff-review.sh` is currently tracked and embedded—requires post-release history analysis.
- `llms.txt`, the automation guide/templates/examples, and the Docker guide agree with their current CLI and workflow entry points. No broken or obsolete reference was found there during static review.

## `AGENTS.md` / `CLAUDE.md` synchronization review

The tracked `HEAD` versions have the same section order and nearly all of the same content. The feature-local Plan Mode block currently visible in `AGENTS.md` is an AMF-generated working-tree addition paired with ignored `.claude/plan.md`; it is not part of the tracked baseline and should not be copied into `CLAUDE.md` as permanent repository guidance.

| Section | Divergence | Recommended alignment |
|---|---|---|
| Title and introduction | Names and audience wording are tool-specific. | Retain the tailored file names/audiences; treat them as intentional equivalents. |
| Build and Run | None. | Keep identical. |
| Runtime Requirements | None. | Keep identical. |
| Architecture introduction | None. | Keep identical. |
| Data Model | Two editorial wording differences (`State is persisted`/`State persisted`, `which directory`/`the directory`). | Choose one wording and copy it to both; there is no semantic reason to differ. |
| App State & Modes | None. | Keep identical and refresh both together when the module map changes. |
| Event Loop & Key Handling | None. | Keep identical; both also need the event-driven scheduling correction identified above. |
| External Tool Managers | `CLAUDE.md` documents `send_keys(session, window, keys)`; tracked `AGENTS.md` omits `keys`. | Use the `CLAUDE.md` form in both; it matches `TmuxManager::send_keys` in `src/tmux.rs`. |
| UI Rendering | None. | Keep identical. |
| Key Handlers | None. | Keep identical. |
| Debug Logging | None. | Keep identical. |
| Key Design Patterns | None. | Keep identical and refresh both when architecture changes. |
| Dependencies | None. | Keep identical; consider pointing to `Cargo.toml` instead of maintaining a partial list if drift becomes frequent. |

Synchronization should compare normalized shared sections in CI or generate both files from one common body with only their titles/introductions templated. A future documentation patch should correct the `send_keys` signature and shared architecture drift in both files in the same commit.

## Project and tool configuration review

- `.amf/config.json` is active project configuration: the release custom session calls `scripts/release.sh`, and the prompt template is loaded by the extension/config merge path. Its `Status Ticker` custom session calls `.amf/test-ticker.sh`, a path that does not exist in the working tree or any reachable git history; this session is therefore a confirmed broken configuration entry.
- `.opencode/commands` is a working symlink to `../.claude/commands`, deliberately sharing the tracked command library between agents. In contrast, `.opencode/settings.json` is a broken symlink to the intentionally untracked/missing `.claude/settings.json`. Current project configuration is `.opencode/opencode.json`; no tracked consumer names the broken compatibility symlink.
- `.claude/session-status-db-migration.md` describes Migration 004 with an entirely unchecked test plan, but the table, CLI command, DB accessors, fallback migration, cleanup paths, and tests are present in `src/` and `scripts/set-session-status.sh`. It is completed implementation planning material stored among active tool configuration and is a consolidation candidate (move to historical documentation or remove after maintainer confirmation).
- `.claude/notes.md` is intentionally minimal but active: the README maps `m` to the memo session and `src/markdown.rs` includes the path in Markdown discovery. Retain it as a scaffold unless the memo feature itself changes.
- The tracked Claude commands are exposed to OpenCode through `.opencode/commands`; the PR commands call their matching `scripts/amf/` helpers. `ai-review.md` depends on `.amf/change-history.json`, which is produced for Vibeless OpenCode sessions by the bundled change tracker. These are runtime consumers, not dead documentation.
- `.github/workflows/release.yml` consumes the release packaging script and release-note sections. `.github/workflows/main.yml` has a confusing test pipeline: `rustc --version && cargo --version |` continues into `cargo test`, piping version output to the test process instead of printing both versions as its comment says. The tests still run, but the step should use three independent commands.
- The reviewed project settings are local (`.amf/`, `.claude/`, `.opencode/`). They do not inject persistent global Claude/OpenCode settings. Source cleanup code still removes historical AMF entries from global Claude settings, consistent with the local-only policy.

## Bundled and operational artifact review

| Artifact group | Consumption evidence | Audit disposition |
|---|---|---|
| `skills/amf-*` | All six skills are embedded in `AMF_SKILLS` and injected into `.claude/skills`, `.opencode/skills`, or `.agents/skills` according to the selected agent; cleanup uses the same list. | Retain. Direct runtime consumers and symmetric cleanup are present. |
| `.opencode/plugins/*.js` | All three plugins are embedded by `src/app/setup.rs`; input-request and sidebar-state are installed for OpenCode, while change-tracker is installed for Vibeless mode. Tests assert install/refresh behavior. | Retain. Direct runtime consumers are present. |
| `plugins/diff-review/scripts/custom-diff-review.sh` | Embedded into the global AMF helper directory and resolved into Claude Vibeless local hook settings; it is the current structured in-app review hook, despite the directory name overlapping the retired legacy viewer wording in the changelog. | Retain; clarify the changelog/current documentation distinction. |
| `scripts/codex-diff-review.sh` and notification/status scripts | Embedded and staged by `ensure_notify_scripts`; Codex/Claude launch and hook paths consume them. | Retain. Direct runtime consumers are present. |
| `themes/opencode/*.json` | Four canonical themes are embedded by `src/theme.rs` and injected into feature worktrees. | Retain. Direct compile-time and runtime consumers are present. |
| `.opencode/themes/*` | Three JSON files are byte-for-byte duplicates of canonical embedded themes. `.opencode/opencode.json` selects `amf-catppuccin`, providing project-local OpenCode use, but Gruvbox is absent and the duplicate README is stale. | Consolidation opportunity, not a removal recommendation: decide whether direct repo OpenCode use must work before AMF starts, then generate/synchronize this directory from the canonical source or document why both copies are maintained. |
| Theme generator scripts | Both standalone scripts generate ten transparent wrapper themes under `.opencode/themes`; their output set and design conflict with the four current full, non-transparent canonical themes. Nothing tracked calls or documents the generators. | Suspected obsolete; require maintainer confirmation and history evidence before removal. |
| `examples/vtcheck.rs` | Auto-discovered Cargo example that reproduces the pane parsing pipeline against a live tmux target. It has no tracked documentation/caller and defaults to a bug-specific session name. | Diagnostic artifact needing confirmation; retain unless the pane-corruption investigation is confirmed closed and the watchdog has no ongoing manual use. |
| Docker no-tmux assets | The guide calls the local bundle/runner scripts; the Dockerfile copies both helper scripts. These validate published release bundles on systems without tmux. | Retain. The workflow is documented and internally complete. |
| Release bundle script | Called by every release build matrix entry and packages the adjacent tmux runtime. | Retain. It is release-critical. |

One cleanup asymmetry is confirmed in code: `ThemeManager::inject_opencode_themes` writes four themes, but `cleanup_opencode_plugins` removes only three and omits `amf-gruvbox.json`. A future fix should derive injection and cleanup from one shared manifest (and extend the test to cover Gruvbox), preventing new managed themes from becoming orphaned.

## Git-history evidence

For this audit, “recent” means changes since `v0.24.0` (2026-06-16) through `HEAD` (2026-06-23), with full-path history also inspected back to the initial commit. A lack of recent commits lowers confidence only when static/runtime consumers are also absent.

| Candidate or discrepancy | History evidence and confidence effect |
|---|---|
| Unused `pretty_assertions` dev dependency | Added in `9951d71` (2026-02-27) with the initial large test suite. The patch added the dependency but no `pretty_assertions` import/use; no later history adds one, and current source has none. This raises removal confidence to high. |
| Theme generator scripts | Added with the first OpenCode themes in `4b75f03` and last changed in `4574100`, both on 2026-03-01. The canonical theme system was overhauled in `21e15fa` (2026-03-06), and Gruvbox was added in `a190b2e` (2026-03-16), without updating either generator. No recent or release workflow use was found. This supports “obsolete” but does not prove absence of manual downstream use, so confidence remains medium. |
| `scripts/test-thinking.sh` | Added in `2d2beec` (2026-03-06) alongside Codex notification/thinking work and never changed. Its only tracked reference is its own usage comment. It may remain a manual diagnostic, so removal confidence is low. |
| Broken `.opencode/settings.json` | The symlink was added in `b7b3d1c` (2026-02-15). Its target `.claude/settings.json` was deliberately untracked by `d241b9c` (2026-03-12); the symlink was not repaired or removed afterward. This gives high confidence that the tracked link is obsolete/broken. |
| Missing Status Ticker command | The `.amf/config.json` entry was added in `bca4321` (2026-06-01) and its metadata was normalized in `2486d5f` (2026-06-21), but `.amf/test-ticker.sh` has never existed in any reachable history. Recent edits show intent to retain the session, so the preferred follow-up is restore the script or remove the entry deliberately, not assume which outcome is wanted. The breakage itself is high confidence. |
| Session-status migration plan | Initially authored as `.claude/plan.md` in `102d094` (2026-04-22), carried through the feature merges culminating in PR merge `4049311` (2026-05-07), and not updated after implementation or the later session-status refinement `bca4321`. This gives high confidence that it is completed historical planning material, while its final archival location remains a maintainer choice. |
| `examples/vtcheck.rs` | Added once in pane-corruption fix `8e40e89` (2026-06-12), with no later changes. Because this is close to the audited release window and the event-loop/pane code continued changing, history argues for retention until maintainers confirm the investigation is closed. |
| Duplicate project/canonical themes | Both copies and their READMEs were introduced together in `4b75f03`; the three common project-local copies were last synchronized during 2026-03-01/06 theme fixes. Gruvbox was added only to the canonical embedded set in `a190b2e`, and only `themes/opencode/README.md` was updated for the later theme design. Neither redundant README has a compile-time/runtime consumer. History confirms deliberate initial duplication followed by drift: remove `themes/README.md` and `.opencode/themes/README.md`, while treating removal of the project-local JSON copies as a separate decision. |
| Gruvbox cleanup omission | The cleanup list dates to `45595fe` (2026-03-07); Gruvbox injection was added later in `a190b2e` without updating cleanup. This sequencing gives high confidence that the omission is accidental. |
| Syntax fixture README | The README and 19 fixtures were added together in `9e5da49` (2026-04-10), yet the README listed only three from its first version. A twentieth fixture was added in `6b316c7` (2026-05-09) without a README update. The incompleteness is high confidence. |
| Fixed-bug backlog text | The backlog and its sole bug were added in `3b8ad2a` (2026-06-21); the bug was marked fixed the next day in `98d13cc`, while the index still says it is currently tracked. Recent history confirms a stale documentation follow-up, not an obsolete backlog feature. |
| README prompt/theme/architecture claims | Prompt-library documentation landed in `558caaf` (2026-06-19) before placeholder support in `v0.26.0`; it was not refreshed for the release. README was touched again in cleanup commit `6cd35e6` (2026-06-23) without resolving prompt/theme drift. Its Architecture Notes duplicate How It Works and agent guidance that had already begun drifting. History and current structure support removing that redundant section rather than maintaining a third architecture summary. |
| Guidance-file drift | `AGENTS.md` and `CLAUDE.md` were last changed together in `1f96119` (2026-05-08), before substantial June module and event-loop changes. History supports updating both in one synchronized documentation change. |
| CI version/test pipeline | The pipeline expression was introduced in `48fb843` (2026-03-04) and is unchanged. It is longstanding rather than recently broken, but shell semantics still contradict its comment; correction confidence is high. |
| Legacy/current diff-review naming | Cleanup commit `6cd35e6` removed `diff-review.sh`, `explain.sh`, `feedback-prompt.sh`, plugin manifest, and hook manifest, but retained `custom-diff-review.sh` and its current native-view hook consumer. History confirms the retained script is not a restoration of the removed vimdiff viewer. |

## End-to-end workflow cross-check

- **Installation and upgrade:** The release workflow produces the asset names consumed by the upgrader and no-tmux installer. The bundle contains `amf` plus a neighboring tmux wrapper/runtime; scripts, plugins, skills, and canonical themes needed after install are `include_str!` data in the binary rather than omitted filesystem dependencies. Upgrade unit tests cover bundle preference, legacy fallback, symlink handling, and large payloads.
- **Release:** `.github/workflows/release.yml` runs tests, builds four targets, and calls `scripts/package-release-bundle.sh` for each. `CHANGELOG.md` release headings feed the release-note step. The release/Docker assets are active and should not be cleanup targets merely because they are outside `src/`.
- **Automation:** All six JSON examples/templates parse as JSON. CLI declarations and `src/automation.rs` correspond to the documented create-project, create-feature, and batch-feature flows; passing tests cover dry-run validation, hook prompts, creation, and failure cases.
- **Themes:** Compile-time embedding, worktree injection, project-local OpenCode selection, and injection tests confirm runtime use. The missing Gruvbox cleanup/test coverage and duplicated/stale project-local documentation remain the only confirmed cleanup gaps in this flow.
- **Syntax parsers:** The README’s on-demand install model matches `src/highlight/`; parser installation/highlighting/startup-validation tests pass. Two fixture files are compile-time inputs, while the broader `docs/syntax-tests` set supports manual diff validation. The cleanup need is documentation/classification, not fixture deletion.
- **Worktrees and local configuration:** Feature startup calls local notification/plugin/skill injection for both the main repo and worktrees. Cleanup is agent-specific, and tests cover local Claude settings, OpenCode plugin refresh/cleanup, Codex hooks, status migration, and worktree lifecycle. No finding recommends removing these runtime assets.
- **Agent sessions:** Claude, Codex, and OpenCode each have explicit launch, transcript/sidebar, notification, and cleanup consumers; Pi intentionally receives no injected AMF skills in the current match. The custom diff-review hook is exercised by current tests and must be retained.
- **Test/documentation consistency:** The passing 615-test suite makes the shared “no tests yet” guidance a high-priority documentation correction. The README’s contributing command (`cargo test`) is the accurate workflow.

## Prioritized findings

Risk describes the impact of acting on a wrong recommendation; confidence describes the strength of the audit evidence. Within each category, high-confidence/low-risk actions come first.

| Rank | ID | Finding | Category | Risk | Confidence | Verification |
|---:|---|---|---|---|---|---|
| 1 | SC-1 | Remove the broken `.opencode/settings.json` compatibility symlink. | Safe cleanup | Low | High | Confirmed |
| 2 | SC-2 | Remove unused `pretty_assertions` from dev dependencies. | Safe cleanup | Low | High | Confirmed |
| 3 | DC-1 | Synchronize and refresh `AGENTS.md`/`CLAUDE.md`, especially the test claim, event loop, module maps, and `send_keys` signature. | Documentation correction | Low | High | Confirmed |
| 4 | DC-2 | Update current README claims and remove its redundant Architecture Notes section. | Documentation correction | Low | High | Confirmed |
| 5 | CO-1 | Use one theme manifest for injection, cleanup, and tests; add Gruvbox cleanup. | Consolidation | Low | High | Confirmed |
| 6 | DC-3 | Correct syntax-fixture and fixed-bug backlog documentation. | Documentation correction | Low | High | Confirmed |
| 7 | SC-3 | Split the CI version-print and test pipeline into separate commands. | Safe cleanup | Low | High | Confirmed |
| 8 | CO-2 | Archive or relocate the completed session-status migration plan. | Consolidation | Low | High | Partially verified |
| 9 | NC-1 | Decide whether to restore the missing Status Ticker script or remove its config entry. | Needs confirmation | Medium | High (breakage) | Partially verified |
| 10 | CO-3 | Retain one corrected theme maintainer guide, remove two redundant theme READMEs, and establish a policy for duplicate theme JSON. | Consolidation | Medium | High (drift) | Partially verified |
| 11 | NC-2 | Confirm whether the two obsolete-looking theme generator scripts have downstream/manual users before removal. | Needs confirmation | Medium | Medium | Partially verified |
| 12 | NC-3 | Confirm ongoing manual use of `scripts/test-thinking.sh` and `examples/vtcheck.rs` before removing either diagnostic. | Needs confirmation | Medium | Low | Unverified externally |

### SC-1 — Broken OpenCode settings symlink

- **Location / scope:** `.opencode/settings.json`; direct OpenCode use in the repository.
- **Category:** Safe cleanup.
- **Evidence:** The tracked symlink targets missing `.claude/settings.json`. That target was deliberately untracked in `d241b9c`; `.opencode/opencode.json` is the current valid project config, and no tracked consumer references the symlink.
- **Recommendation:** Remove the broken symlink. Add a replacement only if a currently supported OpenCode version demonstrably requires a compatibility filename.
- **Risk:** Low; the link cannot currently resolve.
- **Confidence:** High.
- **Verification status:** Confirmed by filesystem resolution, static search, and path history.
- **Expected impact:** One broken tracked entry removed and less confusing OpenCode configuration discovery.

### SC-2 — Unused development dependency

- **Location / scope:** `Cargo.toml`, `Cargo.lock`; test dependency graph.
- **Category:** Safe cleanup.
- **Evidence:** `pretty_assertions` was added with the initial test suite in `9951d71` but was not used in that patch and has no current source/example reference. All 615 tests pass using standard assertions.
- **Recommendation:** Remove `pretty_assertions` from `[dev-dependencies]` and regenerate `Cargo.lock` through Cargo.
- **Risk:** Low.
- **Confidence:** High.
- **Verification status:** Confirmed by current search, addition history, and passing tests.
- **Expected impact:** Removes one direct dev dependency and its lockfile-only transitive crates when no other dependency needs them.

### DC-1 — Shared agent guidance is synchronized but stale

- **Location / scope:** `AGENTS.md`, `CLAUDE.md`; contributor/agent workflows.
- **Category:** Documentation correction.
- **Evidence:** Both files claim there are no tests despite 615 passing tests; both describe the old fixed polling model and incomplete module maps. Their only tracked semantic divergence is the `send_keys` signature, for which `CLAUDE.md` matches current source. Both were last updated together on 2026-05-08, before substantial June changes.
- **Recommendation:** Refresh the shared content from current source, use `send_keys(session, window, keys)`, document the test suite/event-driven loop, and enforce normalized synchronization while retaining tailored titles/intros.
- **Risk:** Low.
- **Confidence:** High.
- **Verification status:** Confirmed section by section against source and test output.
- **Expected impact:** Prevents agents from following outdated architecture and validation guidance.

### DC-2 — README understates shipped features and duplicates architecture guidance

- **Location / scope:** `README.md` prompt-library, bundled-theme, and architecture sections.
- **Category:** Documentation correction.
- **Evidence:** Placeholder/declarative prompt support is shipped and tested, and four non-transparent canonical themes are embedded, including Gruvbox. The Architecture Notes repeat How It Works and `AGENTS.md`/`CLAUDE.md` while already carrying a stale fixed-poll description.
- **Recommendation:** Update the prompt and bundled-theme user guidance, remove the Architecture Notes section, retain the existing pointer to detailed agent guidance, and refresh the “Last updated” date.
- **Risk:** Low.
- **Confidence:** High.
- **Verification status:** Confirmed against source, tests, and release history.
- **Expected impact:** Accurate user-facing capability and configuration guidance.

### CO-1 — Theme injection and cleanup use separate manifests

- **Location / scope:** `src/theme.rs`, `src/app/setup.rs`, theme tests.
- **Category:** Consolidation.
- **Evidence:** Injection writes four managed themes; cleanup removes a hardcoded three-theme list created before Gruvbox was added. Existing injection tests do not assert Gruvbox.
- **Recommendation:** Define one managed-theme manifest consumed by injection, cleanup, and tests; include `amf-gruvbox.json` in cleanup behavior.
- **Risk:** Low.
- **Confidence:** High.
- **Verification status:** Confirmed by source comparison and blame sequencing.
- **Expected impact:** Prevents orphaned injected theme files and future list drift.

### DC-3 — Fixture and backlog indexes are stale

- **Location / scope:** `docs/syntax-tests/README.md`, `docs/backlog/README.md`, `docs/backlog/bug-backlog-plan.md`.
- **Category:** Documentation correction.
- **Evidence:** The syntax README names three of 20 fixture files and does not identify compile-time fixtures. The backlog index says it currently tracks the dashboard `A` bug although that sole entry is marked fixed.
- **Recommendation:** Inventory/classify every syntax fixture and update the backlog index to state that no active bugs are listed (or add current entries).
- **Risk:** Low.
- **Confidence:** High.
- **Verification status:** Confirmed from tracked files, compile-time includes, and history.
- **Expected impact:** Makes manual testing and backlog state understandable without source archaeology.

### SC-3 — CI version output is accidentally piped into tests

- **Location / scope:** `.github/workflows/main.yml` test job.
- **Category:** Safe cleanup.
- **Evidence:** The multiline shell step uses `rustc --version && cargo --version |` followed by `cargo test`; the pipe joins `cargo --version` to the test process despite a comment saying both versions are printed. The expression has been unchanged since `48fb843`.
- **Recommendation:** Put `rustc --version`, `cargo --version`, and `cargo test --workspace --verbose` on independent lines.
- **Risk:** Low.
- **Confidence:** High.
- **Verification status:** Confirmed by shell parsing and blame.
- **Expected impact:** Clear CI logs and straightforward failure semantics.

### CO-2 — Completed migration plan remains in active tool configuration

- **Location / scope:** `.claude/session-status-db-migration.md`; documentation discovery.
- **Category:** Consolidation.
- **Evidence:** Migration 004, DB access, CLI, fallback, cleanup, and tests described by the plan are implemented, while its test checklist remains entirely unchecked. It has not been maintained since the feature merged.
- **Recommendation:** Move it to a clearly historical documentation location with completion context, or remove it if git history is the chosen archive. Do not leave it looking like active work under `.claude/`.
- **Risk:** Low.
- **Confidence:** High that the plan is completed; the preferred archive policy is not established.
- **Verification status:** Partially verified pending maintainer choice of archive/removal policy.
- **Expected impact:** Less stale planning material in active tool configuration.

### NC-1 — Status Ticker configuration points to a nonexistent script

- **Location / scope:** `.amf/config.json`, expected `.amf/test-ticker.sh`; custom-session picker/runtime.
- **Category:** Needs confirmation.
- **Evidence:** The entry calls `bash .amf/test-ticker.sh`; that path is absent from the checkout and all reachable history. The entry itself was edited as recently as 2026-06-21.
- **Recommendation:** Decide intent: restore and track a working ticker script if this remains a repository diagnostic, or remove the broken custom-session entry. Do not silently choose between those outcomes in a cleanup patch.
- **Risk:** Medium because removal could discard an intended developer workflow.
- **Confidence:** High that current configuration is broken.
- **Verification status:** Partially verified; intended workflow is unknown.
- **Expected impact:** Removes a guaranteed runtime failure from the session picker.

### CO-3 — Theme assets and documentation have competing sources of truth

- **Location / scope:** `.opencode/themes/`, `themes/README.md`, `themes/opencode/`, `.opencode/opencode.json`.
- **Category:** Consolidation.
- **Evidence:** Three JSON files are exact duplicates across directories, Gruvbox exists only in the canonical embedded directory, and three README surfaces disagree. `themes/README.md` adds only a directory overview; `.opencode/themes/README.md` describes the obsolete transparent copies; `themes/opencode/README.md` contains the useful build/update instructions. The project config selects a project-local theme, so direct OpenCode use remains a plausible consumer of the JSON files—not of the redundant READMEs.
- **Recommendation:** Keep and correct `themes/opencode/README.md`; remove `themes/README.md` and `.opencode/themes/README.md`. Declare `themes/opencode/` canonical and generate/synchronize `.opencode/themes/` if pre-injection direct use is required; otherwise confirm that use is unsupported before proposing removal of the duplicate JSON files.
- **Risk:** Medium because removing project-local copies could break direct OpenCode startup before AMF injects assets.
- **Confidence:** High that duplication/drift exists; medium on the correct consolidation mechanism.
- **Verification status:** README removal/consolidation is confirmed; JSON asset consolidation remains partially verified pending direct-OpenCode support expectations.
- **Expected impact:** Eliminates manual duplication and contradictory theme guidance.

### NC-2 — Theme generators describe an older system

- **Location / scope:** `scripts/generate-amf-themes.sh`, `scripts/generate-amf-themes.js`.
- **Category:** Needs confirmation.
- **Evidence:** Both produce ten transparent wrapper themes, conflict with the four current full/non-transparent themes, have no tracked caller/documentation, and were not updated through the theme overhaul or Gruvbox addition.
- **Recommendation:** Ask maintainers whether either is used manually/downstream. If not, remove both; if generation is desired, replace them with one deterministic generator for the canonical theme set and test its output.
- **Risk:** Medium because standalone scripts can have untracked consumers.
- **Confidence:** Medium.
- **Verification status:** Partially verified statically and historically; external use unverified.
- **Expected impact:** Removes or modernizes two contradictory maintenance paths.

### NC-3 — Standalone diagnostics lack lifecycle documentation

- **Location / scope:** `scripts/test-thinking.sh`, `examples/vtcheck.rs`.
- **Category:** Needs confirmation.
- **Evidence:** Neither has a tracked caller or external guide. `test-thinking.sh` has been unchanged since its 2026-03-06 addition; `vtcheck` was added with a recent pane-corruption fix and contains a bug-specific default tmux target.
- **Recommendation:** Confirm current manual use with maintainers. Retain and document invocation/purpose if still useful; otherwise remove individually, with stronger caution around the recently added `vtcheck` example.
- **Risk:** Medium.
- **Confidence:** Low because manual diagnostics are inherently invoked outside static paths.
- **Verification status:** External/manual use unverified.
- **Expected impact:** Clarifies whether two standalone utilities are supported tools or one-off debugging residue.

## Recommended follow-up actions

1. Land the low-risk cleanup/correction set: SC-1, SC-2, SC-3, DC-1, DC-2, DC-3, and CO-1, with `cargo test`, `cargo check`, and a CI YAML validation.
2. Resolve NC-1 explicitly by either restoring the Status Ticker script or deleting its entry.
3. Choose documentation/archive policy for CO-2. For CO-3, remove the two redundant theme READMEs and correct the retained guide; separately decide whether direct OpenCode use requires generated project-local JSON copies.
4. Ask maintainers about external/manual use of NC-2 and NC-3 before proposing deletions.
5. Add a lightweight repository audit check for broken symlinks, relative Markdown links, duplicated managed manifests, and synchronized agent guidance.

## Retained uncertainties

- No downstream repository or local manual invocation can be proven absent from this checkout; this particularly affects standalone generators and diagnostics.
- Direct OpenCode startup in the repository may depend on `.opencode/themes` before AMF injection. Current config supports that inference, but no explicit support contract was found.
- The intended Status Ticker workflow is unclear: recent config maintenance supports retention, while the script has never been tracked.
- There is no established policy for completed feature plans—retain in place, move to `docs/`, or rely on git history.
- Interactive tmux/agent behavior, Docker image execution, and multi-platform release packaging were not executed locally; static paths and relevant unit tests passed.
- `cargo build --release` was not run because no compiled behavior changed; debug/all-target/test builds passed.
