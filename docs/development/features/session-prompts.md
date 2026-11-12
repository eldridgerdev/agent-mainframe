# Session initial prompts

`custom_sessions[].initial_prompt` requires an explicit `agent` and nonempty,
NUL-free text. Validate before starting the feature, checking the harness, or
creating a window. The config wizard preserves this field when editing an entry.

The TUI and desktop custom-session creation paths apply the configured prompt
once, after successfully saving the new session. Ordinary restarts and transcript
resumption must not reapply it. This is a creation default, not a persisted
session field or a wrapper argument.

`App::seed_session_compose_draft` resolves the new session's stable ID after save
and stores the draft under the composer's existing tmux session/window key. It
does not change the current view, inspect readiness, or send tmux input. Entering
the new agent's view restores the text through the normal composer path.

Automation accepts prompts for built-in agents and custom entries declaring an
agent. A request prompt overrides the configured default. With `submit_prompt:
false`, creation seeds the composer immediately; with `true`, an explicit request
prompt is queued for harness delivery and no configured draft is left behind.
Queued submissions retain the stable session ID, tmux session/window ownership,
running-state checks, readiness detection, and two-minute timeout.

The desktop owns its drafts in React state. Its custom-session response returns
`draft_prompt`; the frontend preserves it through delayed workspace snapshots
using the pending-session guard, including when `autolaunch` is false.

Drafts and queued submissions are process-local and are lost when AMF restarts.
See [automation](../../automation/README.md#initial-prompts) for user-facing
configuration and delivery semantics. Regression coverage lives in
`app::tests::automation`, `app::tests::feature_sessions`, `gui_sessions::tests`,
and `gui/tests/SessionPicker.test.tsx`.
