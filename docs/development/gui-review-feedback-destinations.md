# Desktop Final Review feedback destinations

The pre-finish summary offers current-feature sessions, a dedicated **Final
Review** agent, another feature's session, and a new companion worktree.
Selecting or previewing a destination has no creation/start effects. **Prepare
destination** separately confirms creation, the exact committed branch basis,
resource warnings and the configured worktree hook choice.

Worktree creation and the confirmed hook run off the GUI coordinator. The open
review retains an operation ID and the created feature/session IDs. **Cancel
pending launch** prevents the later start; it waits for work already running to
settle and retains created checkouts/features. Closing or editing the review is
blocked during setup. Hook failure, stale continuation or worker failure leaves
an explicit recovery message and preserves created resources. If Git creates a
checkout but later settings copying fails, the error names its retained path and
branch for manual recovery. Repairing the input does not automatically recreate,
adopt or delete that uncertain checkout, and no AMF feature row is invented. Replaying the
same successful preparation selects its retained stable session instead of
creating a second companion. The user can select a retained companion as an
existing destination, or manage it from the workspace after pausing the review.

Preparation and completion recheck the source, target, checkout identities,
configuration and exact agent session/pane identity. Global hook choices come
from the same strictly parsed settings snapshot as their revision, so a hook
changed after desktop startup is reflected in the next catalog/preview without
mutating unrelated runtime settings. Another session never
silently replaces a removed target. Companion creation checks both the recorded
branch name and its resolved HEAD; it creates from that confirmed SHA. It does
not copy uncommitted changes. Before creating a companion, it validates the
actual repository settings-copy source and the immutable pinned-base
`.claude/settings.local.json` destination separately. Malformed JSON, unsupported
non-object settings and tracked destination symlinks are refused. Raw source
bytes and physical identity are bound to confirmation and checked again before
creation and after a delayed hook. These are final-read guards, not a lock
against an unrelated writer changing repository files during Git checkout.
Integrating companion commits is outside this increment and remains a terminal
workflow.

Completion uses the shared review-round writer and check/suggestion workflow.
Desktop handoff always returns an unsent draft, even with
`final_review_submit_prompt=true`; the TUI keeps its existing submission policy.
The app appends that draft to any existing destination composer text. For a
different checkout, the draft names the original source's absolute feedback
file and the destination checkout. No destination feedback file is overwritten.

## Validation

Focused Rust regressions cover stale session refusal, pinned branch identity,
resource approval, exact hook choice, background setup/cancellation, stale
post-hook continuation, worker panic, TUI hook warnings and source-aware unsent
handoff. Component/App regressions cover loading/errors, StrictMode replay,
delayed previews, discard protection, synchronous pending guards, resource
approval, setup polling and retained composer text.

Native scenario: `scripts/dev/screenshot/scenarios/gui-review-destinations.txt`.
Its private Git/SQLite/tmux fixture runs offline Claude/OpenCode receivers and
mounts a private `HOME/.amf` while preserving HOME. It checks protected user
configuration/runtime digests before and after. The scenario uses the shared
capture wrapper and required user-namespace probe on hosted runners. No paid
harness or GitHub writes are allowed.

Local capture uses a source-matched debug GUI built with a capture-only
`TAURI_CONFIG` devUrl/CSP override for free port 1440. The ordinary default build
is validated separately, leaving any existing desktop instance untouched.
Hosted capture uses port 1420 on its isolated runner.

The final October 29 local run passed eleven asserted native frames. It changes
the private global hook after GUI startup, then verifies the fresh setup choice,
explicit resource approval, stale-target refusal, cancelled launch recovery,
retained companion and unsent composer with the original feedback path. All
protected configuration/runtime inventories stayed unchanged and owned processes
closed. Earlier diagnostics and intermediate captures are labeled accordingly
and are not final-source evidence.

The final normal Rust milestone passed 3,841 library tests and seven GUI tests
with one existing ignored test. Formatting, strict all-target workspace Clippy,
both default/capture-config builds, the full frontend suite (522 tests in 60
files) and production build passed. Five shared destination tests and 97 GUI
review tests passed separately, including repaired-input retry refusal after
partial Git creation. Real paid-harness delivery and macOS runtime validation
remain open.
