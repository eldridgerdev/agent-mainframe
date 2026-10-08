# GUI screenshot viewer implementation basis

This records the implementation boundaries and verified coverage for
`AMF_PLAN.md`. The GUI viewer and adapters are implemented; authenticated
private-source coverage remains a release gate.
The user confirmed both recommended source policies on 2026-10-07.
The evidence contract is defined in [screenshot-evidence-contract.md](screenshot-evidence-contract.md).

## Existing GUI and integration points

| Concern | Existing implementation | Screenshot integration |
| --- | --- | --- |
| Desktop framework | `gui/package.json`: React 18, TypeScript, Vite, Tauri 2 | Components, API DTOs and styling under `gui/src/`; desktop commands under `gui/src-tauri/` |
| Shared backend | `src/gui_contract.rs`: `GuiHandle` owns a private `App`; `FeatureTarget` and `SessionTarget` carry stable IDs | A narrow GUI adapter calls evidence orchestration under `src/app/`; no filesystem reads or GitHub retrieval in renderers |
| Desktop bridge | `gui/src-tauri/src/main.rs`: registered Tauri commands, `AppState(Mutex<GuiHandle>)`; frontend calls `invoke` | Plan work under the handle lock, run blocking retrieval/decoding outside it, then recheck ownership and selection before applying results |
| PR Triage entry | `gui/src/App.tsx`: Git feature page's **PR Triage** action opens `PrTriagePanel` | Add screenshot browsing within the open PR's toolbar; retain the mounted triage panel and its selected comment and drafts |
| PR selection | `gui/src/PrTriagePanel.tsx`, `gui/src/prTriageApi.ts`: picker, loading and review stages | Scope requests to workflow ID, revision, repository, PR number and resolved head SHA; clear the screenshot selection when the PR changes |
| Feature entry | `FeatureView` in `gui/src/App.tsx`: page header actions, including stopped features | Add feature evidence browsing independently of Git support or running state |
| Session entry | `FeatureView`: `activeTab`, `activeSession` and stable `SessionTarget` | Offer the selected session's evidence; feature browsing must also include historical sessions |
| Workspace entry | `App.tsx` workspace navigation | Retained evidence cleanup must remain reachable when original features or sessions no longer exist |
| Watcher ownership | `src/app/mod.rs`: App owns `FsWatcher`; `src/fswatch.rs`: `notify` dirty flags with timer fallbacks | Evidence reconciliation belongs to App; events are hints, with initial and fallback scans and restart rediscovery |
| Launch attribution | `src/project.rs`: `FeatureSession.id`; `src/tmux.rs`: launch environment already includes `AMF_FEATURE_SESSION_ID` | Use the stable AMF ID, independently of harness resume IDs and tmux window names |
| Deletion | `src/app/feature_ops.rs`: foreground and background deletion, successful completion and failure recovery; `GuiHandle::delete_feature` delegates to this engine | Coordinate cleanup in the shared lifecycle, including background deletion and pending evidence operations |

The GUI refreshes external workspace state through `GuiHandle::refresh_store`
and a two-second frontend workspace refresh. It does not run the TUI event
loop. Evidence worker polling and watcher reconciliation therefore need an
explicit GUI bridge path; adding only a `cli.rs` poll would leave the GUI stale.

PR Triage already uses a plan/read/apply pattern:
`gui_pr_triage::{plan_begin,plan_poll,plan_act}`, `PrTriageReads::run`, and the
matching `*_prefetched` functions. Tauri wraps the read in `spawn_blocking`.
Reuse this pattern without coupling the screenshot viewer's lifetime to
`AppMode::PrReview`; local evidence must also work while other feature screens
are open. Give evidence requests their own selection and cleanup generations.

`PrReview` currently exposes the head SHA, branch, comment IDs and bodies,
including replies, but not the PR description. `GhCli::pr_meta` can retrieve
the description, title and changed-file list. The remote adapter must also
resolve the PR head repository for fork-relative image references. Existing
`TriageGithub` and `GithubTransport` seams support offline tests; extend or
compose an evidence-specific read boundary rather than bypassing them in UI
code.

`gui/src/Markdown.tsx` intentionally renders a small Markdown subset and does
not parse links or images. It is not an image-discovery parser. Evidence
extraction should be a backend transformation over original source bodies.

Both desktop CSP configurations in `gui/src-tauri/tauri.conf.json` permit
images only from `'self'` and `data:` and prohibit external frames. Initial
image transport should use bounded, validated image data from backend commands.
Remote downloads and authenticated gallery browsing cannot be implemented by
embedding external pages in the WebView. Browser opening needs a narrow backend
operation accepting validated HTTP(S) URLs; no general shell permission is
needed in `capabilities/default.json`.

## Source policies before adapters

The following are implementation rules for the unresolved technical details.
The product choices in the last subsection were confirmed by the user.

### PR markup, references and provenance

- Parse CommonMark inline and reference images, image destinations inside
  links, and literal HTML `<img src>` inside PR descriptions, conversation
  comments, review summaries, inline review comments and replies. Exclude
  code blocks, inline code, HTML comments and scripts. Do not execute HTML or
  expand CSS images, `srcset`, JavaScript, or arbitrary embeds. Include direct
  HTTP(S) image links as candidates, confirming format from bounded bytes.
- Resolve repository-relative references in PR bodies and comments from the
  PR head repository root at the fetched head commit, including fork PRs.
  Comment file anchors do not change that base. An unavailable/deleted head
  repository blocks relative files and reports a source notice while independent
  attachments, galleries and Actions sources remain available. Reject paths escaping the
  repository after URL decoding and normalization. Fragment-only links are
  not image sources.
- For explicit GitHub `blob`/`raw` URLs, resolve the specified revision to a
  commit once and retain it in provenance. Do not silently substitute the
  checkout's branch, default branch or a newer PR head. Refresh is a new
  projection if the head changes.
- Repository-file identity is host, repository, resolved commit and decoded
  path. Attachment identity is its original attachment URL/identifier, not a
  temporary redirect URL. Keep every description/comment origin even when
  several references resolve to one image. URL fragments do not create new
  images; preserve query parameters unless the source adapter can establish
  they are temporary retrieval credentials.
- Label non-repository attachments with their source and the PR head at
  discovery. That head describes discovery context, not proof the image was
  produced from that commit. Local files must never satisfy a failed remote
  repository-file lookup.

### Retrieval, authentication and coverage gates

- Reuse `gh` authentication for GitHub API reads, including private repository
  contents and Actions artifacts. Public external images and galleries receive
  no GitHub credentials. Bound time, bytes and redirect depth; restrict protocols
  to HTTP(S), prevent credential forwarding to another origin, and prevent
  remote URLs from reaching local/private network addresses.
- Renew source-specific retrieval URLs from stable identities on retry. Do
  not persist signed URLs as identities or include credentials in logs or
  frontend errors. GitHub documents the repository contents `ref` parameter
  and the need to obtain fresh download URLs in its
  [contents API](https://docs.github.com/en/rest/repos/contents).
- Test private GitHub attachments separately from private repository contents.
  Successful contents API access does not demonstrate attachment access.
  The release gate requires authenticated retrieval of representative private
  attachments and artifacts, in addition to offline adapter tests. If an
  attachment requires an unavailable authentication mechanism, surface an
  authentication/retrieval error and record the coverage gap. Protected-gallery
  browser fallback does not apply to attachments.
- No live authenticated private-source validation has been performed during
  this investigation. Do not mark that coverage complete based on API
  documentation or fixtures.
- Local and remote sources share bounded format validation, decoding and
  thumbnail behavior. The evidence contract defines the format, archive, path
  and processing limits enforced by the implementation.

### Confirmed product choices

**Actions default:** the policy is the latest completed run associated
with this PR whose `head_sha` matches the current PR head and whose unexpired
artifacts contain supported images, across workflows and conclusions. Failed
runs can contain useful visual evidence. There must be an explicit
empty state when no default-eligible run exists.

List older associated runs with their status,
commit, workflow and artifact availability, including expired/empty states.
Paginate rather than assuming one page. PR association must be established
from run metadata; branch-name equality alone is insufficient. Keep run ID,
attempt, artifact ID and archive member path in provenance and distinguish
reruns. A newer empty or running run must not silently claim older evidence.
GitHub exposes run and attempt identities in the
[workflow runs API](https://docs.github.com/en/rest/actions/workflow-runs),
and artifact IDs, expiration and run metadata in the
[artifacts API](https://docs.github.com/en/rest/actions/artifacts).

**Gallery boundary:** the policy is direct image links plus an explicit
versioned JSON image manifest, with unsupported public and protected galleries
opening in the browser. The manifest schema and discovery URL convention
are specified in the evidence contract. No crawling,
JavaScript execution or cookie import is proposed.

The repository's current screenshot publisher produces a private Cloudflare
Pages gallery and links it in a marked PR-body region. Its HTML is built by
`scripts/dev/screenshot/build_static_gallery.py`; this is a known protected
gallery/browser case, not evidence of a universal gallery API. This feature
does not require modifying the capture engine or publishing screenshot proof.

## Validation commands and acceptance boundary

Use Node 22.12+ and stable edition-2024 Rust. This workspace includes the
`amf-gui` Rust package, so workspace tests and Clippy also cover its desktop
commands. Linux requires GTK 3, WebKitGTK 4.1 and the other system prerequisites
already installed in `.github/workflows/main.yml` and `release.yml`.

```sh
# Frontend setup and focused existing surface checks, from gui/
npm ci
npm test -- tests/PrTriagePanel.test.tsx tests/App.test.tsx tests/SessionControls.test.tsx
npm run build

# Frontend milestone, from gui/
npm test

# Backend focused checks, from repository root
cargo test --locked gui_pr_triage
cargo test --locked gui_contract
cargo test --locked app::tests::feature_sessions
cargo test --locked app::pr_review

# Code milestone, from repository root
cargo build --locked
cargo build --locked -p amf-gui
cargo test --workspace --locked
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings

# Native development launch, from gui/
npm run tauri dev
```

Frontend component tests use Vitest, jsdom and Testing Library with mocked
Tauri `invoke`; they do not establish real desktop IPC behavior. Backend tests
use temporary repositories/databases and mocked operations. Final acceptance
must exercise actual Tauri commands in an isolated desktop instance, including
late results after switching PR/session/run and closing the viewer. No request
to capture or publish visual proof has been made for this task.

Investigation environment: Node 22.20.0, npm 10.9.3, Rust 1.99.0, GTK 3.24.33
and WebKitGTK 2.50.4; `tmux` and `gh` are present. Check results are recorded
in the local `AMF_PLAN.md` progress notes.

## Implemented coverage and acceptance

The shared viewer is reachable from PR Triage's **Screenshots** action, the
feature header's **Screenshots** and **Session screenshots** actions, and the
workspace's **Validation screenshots** navigation. The workspace entry includes
historical evidence whose original sessions/features have been deleted.

Claude receives an appended launch system prompt. Codex receives a transient
`developer_instructions` override preserving existing instructions from its
configuration layers, selected profile and CLI override. Neither delivery path
writes screenshot destinations into shared harness configuration. See the
[official Codex configuration reference](https://developers.openai.com/codex/config-reference/)
for the session instruction setting. Tests use representative completion
manifests for both harnesses, rather than invoking paid agents; guidance tests
do not prove future agent compliance.

Source adapters support the markup and revision rules above. Public manifests
must be explicitly linked as JSON, or labelled as a gallery, screenshot or
visual-validation link; arbitrary documentation links are not probed. There is
no HTML gallery scraping. Protected or unsupported galleries offer browser
opening. Unsupported images and failed attachment reads remain recoverable
errors in the gallery, without silently substituting local files.

Actions defaults search completed current-head runs across workflows and
conclusions, ordered by `run_started_at` (falling back to creation time), including
reruns discovered on later pages. The run selector also exposes older PR runs.
Artifacts are identified by repository, run ID, artifact ID and archive member.
The current run attempt is shown as context. GitHub's artifact listing does not
bind each artifact to a producing attempt; the UI explicitly says that attempt
attribution is unavailable instead of claiming old artifacts came from a rerun.
Invalid image members produce notices without hiding valid neighbors. Unsafe
archive paths reject the entire archive. Limits may omit later evidence and are
reported to the user; this is not unlimited repository history browsing.

The native acceptance check runs through the real WebKit/Tauri bridge with
isolated XDG configuration/state, temporary ownership records and offline `gh`.
It does not capture or publish screenshot proof. On 2026-10-07 it passed:

- Historical Claude/Codex ownership and image transport.
- Fit/original sizing, zoom, previous/next navigation, thumbnail focus restoration,
  reopen and replacement discovery.
- Rejection of delayed actual IPC results after producing-session changes.
- Pinned repository image retrieval through `gh`, preserving the selected PR
  comment and unsent reply when closing the viewer.
- Rejection of delayed source reads after close, run changes and switching PR
  #12 to #11.
- Explicit cleanup removing only the selected scope and retaining its neighbor.
- Browser opening through an isolated `xdg-open` fixture using the original
  gallery URL.
- Live public GitHub user-attachment retrieval, redirects and thumbnail decoding
  through the production HTTP adapter, using the README attachment as a fixture.
- Live public Actions artifact download, redirect, archive validation and
  thumbnail transport (artifact 11515311988). This proves the public download
  path; PR/run association and default policy are covered by source fixtures.

Reproduce on Linux with an X11 display, WebKitGTK/GTK development prerequisites,
`/usr/bin/python3` with `websocket-client`, and `bwrap` if native Claude versions
are installed. Port 1420 must be free. The script owns and cleans up its Vite/GUI
process groups and temporary state; it never invokes a paid harness or writes to
GitHub:

```sh
cargo build --locked -p amf-gui
/usr/bin/python3 scripts/dev/check-gui-screenshots.py /tmp/amf-gui-screenshot-check
```

The optional `--public-attachment` flag repeats acceptance and additionally reads
the public README attachment using existing `gh` authentication. AMF state remains
isolated; only this optional read uses the existing GitHub authentication config.
`--public-artifact <id>` adds a live artifact metadata/download read from this
public repository; choose an unexpired screenshot artifact. Its repository and
commit metadata are used by the otherwise isolated PR fixture. These optional
checks do not invoke or publish captures. The JSON report is text-only.
Offline native acceptance and source fixtures do
not establish authenticated private attachment/artifact retrieval. That live
coverage is still pending a private PR fixture. GitHub Enterprise hosts are not
supported by this adapter. macOS browser launching is implemented but native
acceptance in this worktree was on Linux.

Local milestone checks on 2026-10-07 passed the locked CLI and GUI builds,
normal parallel workspace tests (3,394 core and 7 GUI backend tests; one existing
live GitHub test ignored), formatting and strict all-target Clippy. The focused
screenshot suite passed 28 tests; affected feature-session and PR-review suites
passed 98 and 93 tests respectively. Frontend TypeScript/Vite build and all 244
frontend tests passed. Vite retains its existing bundle-size advisory.
