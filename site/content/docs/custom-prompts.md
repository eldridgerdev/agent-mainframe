+++
title = "Overriding AI Prompts"
description = "Edit the exact prompts AMF sends for headless AI calls."
weight = 70
+++

Behind the plan interview, Learning Mode, the final-review diff helpers, the
AI PR review, and the review-memory bootstrap/compaction, AMF makes one-shot
("headless") AI calls with prompts it builds for you. Press `E` on the
dashboard — or `Ctrl+Space`, then `E` from a session — to open the
**prompt-override manager**. It lists every template with its effective
source (`built-in`, `feature`, `project`, or `global`) and `[F][P][G]` flags
for which scopes already carry an override.

`Enter` or `e` opens an editor on the effective template. `Ctrl+S` moves to a
scope picker, then a harness picker, then saves:

| Scope | Where it lives | Applies to |
| --- | --- | --- |
| **This feature** | `amf.db` | just this checkout |
| **This project** | `amf.json` `prompt_overrides` key | the repo — committed, shared with everyone |
| **Global** | `amf.db` | every project on this machine |

The nearest scope wins — feature → project → global → built-in — and within
the winning scope a per-harness template beats the shared one. `d`, `d`
clears the effective override. Templates carry visible `{% raw %}{{token}}{% endraw %}`
placeholders that AMF re-fills with live context (the diff, the question,
the interview answers) each time the prompt runs. **There is no
validation**: if you delete a required token or add one AMF does not
supply, it is saved and rendered exactly as written.

Project-scope overrides sit in `amf.json` under `prompt_overrides`, keyed by
the stable prompt id (shown in the manager):

{% raw %}
```json
{
  "prompt_overrides": {
    "pr_review.ai_review": {
      "template": "You are reviewing a diff... {{annotated_diff}} ..."
    },
    "learning.answer": {
      "template": "shared text with {{question}}",
      "harnesses": { "codex": "codex-specific text with {{question}}" }
    }
  }
}
```
{% endraw %}

Before each user-initiated headless call, a **pre-call notice** names the
prompt and target harness: `v` shows the exact rendered prompt, `e` jumps to
the manager for that prompt, `Enter` makes the call, `Esc` cancels it.
Calls that run without you watching — the Learning Mode answer queue and
session summaries — announce with a toast instead of the modal.
