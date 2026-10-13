# Staged desktop release readiness

Evidence snapshot: 2026-10-12. This report reconciles the staged desktop scope
with its review queue. It is not a release approval or a claim that every TUI
workflow has desktop parity. The [interface guide](../interfaces.md) and
[workflow inventory](../backlog/amf-gui-workflow-inventory.md) describe the
supported surface. Older native-proof and installed-package status statements
in guides/inventory must be read with the pinned evidence below.

## Main baseline and unmerged work

The audited main baseline is
[`17b27fce`](https://github.com/eldridgerdev/agent-mainframe/tree/17b27fce1b1df31c442294c21cccaa24066c4ecc).
It already contains the two-executable Cargo workspace, shared SQLite/tmux
coordination, session terminals and lifecycle, scoped TODOs, planning, Learning,
diffs, review and triage, prompt browsing, themes, dormancy and bookmarks.
The database uses checked saves; the GUI refreshes external workspace changes
and detaches its terminal view without stopping the underlying session. See
[store coordination](../../src/db/store.rs),
[GUI workflow boundary](../../src/gui_contract.rs) and
[terminal attachment ownership](../../src/gui_terminal.rs).

Every PR below is **open and unmerged** at this snapshot. A successful gallery
establishes the asserted scenario on its pinned ref, not an integrated release
containing the entire queue. New settings and fixes remain pending capabilities
until reviewed and merged. Package rows exercise the explicitly named released
binaries rather than proving new source behavior. The #726 row pins the original
guide review point; its refreshed illustrative gallery builds the updated docs
head with unchanged application sources.

“Five green” means the pinned PR head passed Test (Linux), Test (macOS), Lint,
GUI frontend and Dependency audit. macOS Rust CI is not native desktop proof.
“Stacked” means those product jobs do not run against its feature-branch base;
local milestone checks pass as recorded in that PR, and main-based CI is still
required after retargeting. Every linked gallery completed strict publication.

| PR / pinned ref | Evidence and bounds | Product CI / base | Gallery |
| --- | --- | --- | --- |
| [#724](https://github.com/eldridgerdev/agent-mainframe/pull/724) · [`dc8debc5`](https://github.com/eldridgerdev/agent-mainframe/commit/dc8debc5fa5da1d8193ec6e195942a2431b48095) | Dormant-list delete: cancel, TODO ownership, stale refusal and ordinary-checkout preservation; 7 native frames. | Five green / main | [724](https://pr-724.amf-screenshot-previews.pages.dev) |
| [#725](https://github.com/eldridgerdev/agent-mainframe/pull/725) · [`aa78cb98`](https://github.com/eldridgerdev/agent-mainframe/commit/aa78cb989a49c47b93b77befc33082a09cc9f590) | Global allowed-agent editor: scoped save, draft/stale guards and effective session choices; 4 native frames. | Five green / main | [725](https://pr-725.amf-screenshot-previews.pages.dev) |
| [#726](https://github.com/eldridgerdev/agent-mainframe/pull/726) · [`ad8521ea`](https://github.com/eldridgerdev/agent-mainframe/commit/ad8521ea4b9f95c7760f8f417e4263b73b709095) | Paired-interface and upgrade guide; 4 unchanged bookmark/navigation illustrations. This row pins the original guide, before this report. | Five green / main | [726](https://pr-726.amf-screenshot-previews.pages.dev) |
| [#727](https://github.com/eldridgerdev/agent-mainframe/pull/727) · [`f6e43af0`](https://github.com/eldridgerdev/agent-mainframe/commit/f6e43af08750e3cf940f7048a8cec59168df6663) | Private screenshot tmux ownership; unrelated-session survival/failure regressions and 2 TUI frames. | Five green / main | [727](https://pr-727.amf-screenshot-previews.pages.dev) |
| [#728](https://github.com/eldridgerdev/agent-mainframe/pull/728) · [`dc93d0fe`](https://github.com/eldridgerdev/agent-mainframe/commit/dc93d0fec86a789873a429b1bdaaabc0b6d05e0b) | Context settings: exact u64 limits, validation, stale/draft protection and refreshed policy; 6 native frames. | Stacked / `gui-agent-defaults` | [728](https://pr-728.amf-screenshot-previews.pages.dev) |
| [#729](https://github.com/eldridgerdev/agent-mainframe/pull/729) · [`675e330d`](https://github.com/eldridgerdev/agent-mainframe/commit/675e330dd39c1f9dbe7489e1f1b9ecf43f65e121) | Lossless startup config migration with unknown-field/permission regressions; 2 native TUI frames. | Five green / main | [729](https://pr-729.amf-screenshot-previews.pages.dev) |
| [#730](https://github.com/eldridgerdev/agent-mainframe/pull/730) · [`af9e0d0c`](https://github.com/eldridgerdev/agent-mainframe/commit/af9e0d0cd17590f8e128c00cf7c5ebf26486b71b) | Prompt-library scopes, inputs, external edits, cancellation and unsent append; 6 native frames. | Five green / main | [730](https://pr-730.amf-screenshot-previews.pages.dev) |
| [#731](https://github.com/eldridgerdev/agent-mainframe/pull/731) · [`f134fa28`](https://github.com/eldridgerdev/agent-mainframe/commit/f134fa28d70bb953cd6644eea16a40874cc2fcc7) | Planning pre-call cancel restores editable state; scoped TODO/hooks, Full/Quick Plan, resource consent and unsent handoff; 17 native frames. | Five green / main | [731](https://pr-731.amf-screenshot-previews.pages.dev) |
| [#732](https://github.com/eldridgerdev/agent-mainframe/pull/732) · [`e20ce623`](https://github.com/eldridgerdev/agent-mainframe/commit/e20ce62351ea486fd46370f7d00775c582b0766f) | Official v0.48.0/v0.49.0 Debian GUI + musl TUI, both update orders, private dpkg root and live-session continuity; 5 native frames. | Five green / main | [732](https://pr-732.amf-screenshot-previews.pages.dev) |
| [#733](https://github.com/eldridgerdev/agent-mainframe/pull/733) · [`0f72647a`](https://github.com/eldridgerdev/agent-mainframe/commit/0f72647a39e6e8180cead043015ea3ccc0954c65) | Learning answers/reopen/follow-up, saved TODO notes, resource consent and unsent editing handoff; 11 native frames. | Five green / main | [733](https://pr-733.amf-screenshot-previews.pages.dev) |
| [#734](https://github.com/eldridgerdev/agent-mainframe/pull/734) · [`048a84b3`](https://github.com/eldridgerdev/agent-mainframe/commit/048a84b33062496529868bd6f25016eb887b791f) | Final Review batch confirmation, stale refusal, partial failure, reopen and retry; 8 native frames. Also corrects capture-head metadata. | Five green / main | [734](https://pr-734.amf-screenshot-previews.pages.dev) |
| [#735](https://github.com/eldridgerdev/agent-mainframe/pull/735) · [`3c6e8fe1`](https://github.com/eldridgerdev/agent-mainframe/commit/3c6e8fe1d685d77dc4e7b35074d4774c6be4a281) | Actual offline OpenCode hook approve/reject/cancel, reason persistence, stale/dead request refusal; 9 native frames. | Five green / main | [735](https://pr-735.amf-screenshot-previews.pages.dev) |
| [#736](https://github.com/eldridgerdev/agent-mainframe/pull/736) · [`c075d373`](https://github.com/eldridgerdev/agent-mainframe/commit/c075d37319d257c567224f40a04f63a90bec6aae) | Official released AppImages, both update orders and existing-server continuity; 5 native frames. Fresh-server Python failure remains separate. | Stacked / `gui-linux-package-validation` | [736](https://pr-736.amf-screenshot-previews.pages.dev) |
| [#737](https://github.com/eldridgerdev/agent-mainframe/pull/737) · [`5b3ff8fb`](https://github.com/eldridgerdev/agent-mainframe/commit/5b3ff8fb2693aec7eee219b9cfa61241b24a0cbc) | Global Feature presets: all fields, preserved unknown data, draft/stale guards, effective choices and explicit unsupported Remote Control intent; 8 native frames. | Stacked / `gui-agent-defaults` | [737](https://pr-737.amf-screenshot-previews.pages.dev) |
| [#738](https://github.com/eldridgerdev/agent-mainframe/pull/738) · [`b6842320`](https://github.com/eldridgerdev/agent-mainframe/commit/b6842320f716273d34351ac4360b21159f4eb713) | Bundle-owned Python environment removal at tmux child boundary; unchanged main fails the regression, patched source GUI passes 2 native frames with released AppRun variables. Actual source-built AppImage check remains pending. | Five green after Linux retry / main | [738](https://pr-738.amf-screenshot-previews.pages.dev) |
| [#739](https://github.com/eldridgerdev/agent-mainframe/pull/739) · [`f0e3477d`](https://github.com/eldridgerdev/agent-mainframe/commit/f0e3477d933c6d2562b19a2d8249ce7e72fa1bce) | Project agent availability, inheritance, legacy config preservation and fresh allowed/installed new-feature choices; 8 native frames. Compatible with #737's preset picker; combined integration still requires validation. | Stacked / `gui-agent-defaults` | [739](https://pr-739.amf-screenshot-previews.pages.dev) |
| [#740](https://github.com/eldridgerdev/agent-mainframe/pull/740) · [`1785d657`](https://github.com/eldridgerdev/agent-mainframe/commit/1785d657f550288f64c7e831b9c85b1c6662be7f) | Explicit edit-claim unlock; deterministic retained-file-clone regression fails before and passes after the fix, preserving held-claim exclusion. Existing native Claude hook behavior illustrated in 8 frames; screenshots do not reproduce the lock race. | Hosted checks pending / main | Publication pending |

Required fixes and their integration status:

- **Fresh-server AppImage Python environment:** narrow removal of bundle-owned
  Python variables at the tmux child boundary. The negative probe fails on
  unchanged main and both released AppImages; #736's successful existing-server
  upgrade proof does not resolve that failure. #738 passes the source-GUI proof
  with released AppRun variables and its gallery is published. A source-built
  debug AppImage check and final helper ref remain pending; the independent
  hosted lock-test failure and retry are recorded below.
- **Project agent availability:** #739 adds the scoped project editor and fresh
  allowed/installed choices before ordinary new-feature creation, including
  loading/error/empty states. Eight native frames pass and its gallery is
  published. Its filtered agents prop is compatible with #737's preset picker;
  their combined integration still needs candidate validation.
- **Supervised-edit lock release:** #738's first hosted Linux suite failed the
  existing published-decision retry test. Its unmodified normal parallel Linux
  rerun passed, so #738 now has five green checks. This does not eliminate the
  identified inheritance race: separate #740 deterministically fails the
  retained-clone regression before the fix and passes after explicit unlock.
  #740's full parallel suite, formatting, strict Clippy, 374 frontend tests/build
  and both executables pass locally. Hosted checks/gallery remain pending.

## What the native proof establishes

The source scenarios use real Tauri IPC with private configuration, SQLite,
Git/worktrees and tmux. They run offline harness fixtures and unpaid receivers;
unsent composer assertions do not claim a paid agent completed a task. OpenCode
proof invokes the shipped hook logic from a temporary module with exactly three
asserted filesystem-path substitutions; it does not run a live paid harness or
write global hook directories. Local desktop captures use WSLg. Hosted scenarios
build their requested source ref; package scenarios verify pinned release
payloads instead.

The original capture workflow labels artifact metadata with the dispatch
workflow's main SHA. Requested-ref checkout logs, rather than that label, identify
these captures. #734 corrects future metadata with `git rev-parse HEAD`, but the
workflow definition does not change until merged. Checkout logs have independently
confirmed the final heads for #731, #733, #734, #735, #737 and #738; the gallery URLs
are mutable PR aliases, so reviewers should also retain the pinned commit links.

The package proof is deliberately bounded: official x86_64 v0.48.0/v0.49.0 GUI
and musl TUI bundles, Ubuntu 22.04/WSLg, both independent update orders, additive
schema 43 → 45, original session/pane/process/server identities, and an unchanged
counterpart executable. Debian installation uses a private fakeroot/dpkg root
with host GTK/WebKit dependencies and `--force-depends`. AppImages use
`--appimage-extract-and-run` and an already-running private tmux server. The
v0.48.0 GNU TUI requires GLIBC 2.39 and failed its Ubuntu 22.04 preflight; that
combination is not counted as validated. This is evidence for these versions,
not arbitrary older/newer compatibility or concurrent feature mutation. The
Python fix is unmerged source work; it does not repair already published
v0.48.0/v0.49.0 AppImages.

## Remaining release gates

| Gate | Required evidence or decision |
| --- | --- |
| Finish required source fixes | Finalize actual fresh-server AppImage evidence and hosted #740 checks/proof, with pinned refs above. |
| Review and integrate | Review all queued changes, resolve stacked dependencies (#728/#737/#739 on #725; #736 on #732), retarget to main and run hosted product CI on the final heads. No merges are authorized by this report. |
| Validate the integrated candidate | Run the normal parallel locked workspace suite, formatting, strict all-target Clippy, frontend tests/build and both executable builds on the actual combined candidate. Replay relevant isolated native scenarios; independent branch counts cannot substitute for this run. |
| Native macOS | Validate Finder-launched PATH discovery, terminal attach/input/detach, supported workflow interactions and independent updates on Apple Silicon. Hosted Rust tests and a built DMG do not establish these behaviors. |
| Linux distribution/install boundaries | Validate actual dependency resolution/system install and desktop launch, FUSE AppImage mounting, and other shipped architectures. Private roots/extract-and-run do not establish them. |
| Signing and distribution policy | Resolve or explicitly retain the documented ad-hoc/unnotarized macOS preview limitation; Developer ID/notarization is not proven. Native Windows remains outside the staged scope; Windows use is WSL2/WSLg. |
| Publish a supported pair | Select candidate versions and release notes, preserve the maintained TUI and separate GUI downloads, review the shared-store upgrade/backup guidance, then obtain the separate merge/release decision. No release is authorized here. |

Advanced TUI parity remains explicitly deferred: broader harness setup,
prompt-template authoring/export, advanced PR Review authoring/AI explanation,
and additional triage/feedback destinations. Remote Control presets are stored
for TUI use; desktop creation requiring them remains visibly blocked until an
explicit opt-out. TODO preset selection is also outside #737. These limits do
not become first-release prerequisites merely because a larger inventory parent
remains open.

This document changes no application behavior. Its four bookmark/navigation
illustrations are existing baseline UI examples, not proof of the queued
features, installation boundaries or native macOS operation.
