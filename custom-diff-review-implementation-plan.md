# Custom Diff Review Implementation Plan

## Goal

Make non-review-mode diff review configurable between the legacy popup flow and the new custom diff viewer, with the custom viewer as the default when the config is omitted. Then bring the custom diff viewer to parity with the current diff review tool's actions.

## Steps

1. Add a global viewer selector to `AppConfig` in `src/app/mod.rs`, likely as an enum such as `DiffReviewViewer { Custom, Legacy }`, with `Custom` as the default. Because `AppConfig` already uses `#[serde(default)]`, omitted config will naturally fall back to the new default. Add serde/default coverage in `src/app/tests.rs`.

Commit checkpoint: `config: add diff review viewer setting with custom default`

2. Make generated Claude hook configuration honor that setting in `src/app/setup.rs`. Keep the existing `plugins/diff-review/scripts/diff-review.sh` path for `Legacy`, and generate the custom-viewer path when `Custom` is selected. Since this behavior is written into per-workdir hook files, add a refresh pass for existing Claude features on startup so changing `~/.config/amf/config.json` takes effect without recreating features.

Commit checkpoint: `hooks: wire diff review viewer config into generated Claude hooks`

3. Split or rename the current custom-viewer state so it is explicitly a diff-review flow rather than a `change-reason` flow. Today that modal is `ChangeReasonPrompt` in `src/app/state.rs`, rendered from `src/ui/dialogs/hooks.rs`, and handled by `src/handlers/change_reason.rs`. Either rename that path or introduce a dedicated diff-review state next to it so the notification and response semantics remain clear.

Commit checkpoint: `ui: promote custom diff review to a dedicated app state`

4. Change notification routing in `src/app/notifications.rs` so `diff-review` no longer auto-responds `proceed` when the custom viewer is enabled. That auto-proceed path is the current legacy behavior. Instead, selecting or receiving a `diff-review` notification should open the custom viewer with the structured diff payload and only reply after the user chooses an action. Preserve the current auto-proceed behavior for the legacy viewer path.

Commit checkpoint: `notifications: route diff-review events into custom viewer`

5. Bring the custom viewer to feature parity with the legacy popup in `plugins/diff-review/scripts/diff-review.sh`. The legacy options are:

- approve/proceed
- reject with feedback
- explain
- cancel

The current TUI dialog only supports accept, skip, and reject-with-inline-reason. Extend it so the custom viewer exposes all four actions, with response payloads that match the hook's expectations. For `explain`, prefer reusing the existing explain script or its behavior instead of inventing a separate explanation path.

Commit checkpoint: `diff review: add explain, cancel, and feedback parity in custom viewer`

6. Finish with tests and manual validation. Add unit coverage for omitted-config defaults, explicit config deserialization, hook generation per viewer setting, notification routing, and approve/reject/cancel response serialization. Then run `cargo test` and `cargo check`, plus a smoke test for both modes: `Custom` opens the TUI diff viewer and `Legacy` still opens the tmux/neovim popup.

Commit checkpoint: `test: cover diff review viewer config and routing`

## Notes

The two design constraints that matter most are:

- This setting affects generated Claude hook files, so existing worktrees need a refresh path.
- The current custom viewer is still modeled as a `change-reason` flow, which is misleading for the behavior you want and will complicate notification handling unless it is split or renamed early.
