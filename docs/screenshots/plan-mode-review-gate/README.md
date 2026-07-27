# Plan-mode synthesis review gate

Captured with the `/amf-screenshot` workflow from an isolated AMF instance
against a throwaway repository. The fixture exercises the real plan-mode UI
without reading or modifying user projects, the normal AMF database, or any
agent session.

## 1. Rendered plan review

The synthesized implementation plan opens as rendered markdown. The review
gate exposes scrolling, edit, regenerate, accept, and abort actions.

![Rendered plan review](01-rendered-plan-review.png)

## 2. Raw markdown editor

Pressing `e` opens the plan in the shared text editor. `Ctrl+S` saves the edit
back to the preview; `Esc` discards the edit.

![Raw markdown plan editor](02-raw-markdown-editor.png)

## 3. Saved edit preview

After saving, the rendered preview reflects the edited markdown while the
feature remains paused behind the acceptance gate.

![Edited plan returned to the rendered preview](03-saved-edited-preview.png)

## 4. Abort confirmation

Pressing `Esc` from the preview asks for confirmation before abandoning the
interview and its deferred feature launch.

![Plan review abort confirmation](04-abort-confirmation.png)
