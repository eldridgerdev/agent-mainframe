---
name: amf:add-prompt
description: >
  Add a declarative prompt template to this AMF workspace. Use when
  the user wants a reusable, version-controllable prompt — with
  optional {{placeholder}} slots and select menus — that shows up in
  the prompt library picker (leader+P in a session, or L on the
  dashboard) so they can inject it into an agent instead of retyping
  it.
allowed-tools: Bash(cat *) Bash(mkdir *) Edit(amf.json) Bash(test *)
argument-hint: "[prompt name]"
---

## Current config

!`cat amf.json 2>/dev/null || echo "{}"`

## Task

Add or update an entry in the `prompt_templates` array in
`amf.json`. Create the file (and the array) if it doesn't
exist. These declarative templates are read-only in the picker (the
user duplicates one with `y` to get an editable copy), and they're
merged in alongside the user's SQLite-backed `User` templates.

## PromptTemplate schema

```json
{
  "name": "Write a commit message",
  "description": "Conventional-commit style message from staged diff",
  "tags": ["git", "writing"],
  "body": "Write a {{type}} commit message for these changes:\n\n{{summary}}",
  "placeholders": [
    {
      "key": "type",
      "label": "Commit type",
      "kind": "select",
      "options": ["feat", "fix", "docs", "chore"],
      "required": true
    }
  ]
}
```

| Field | Required | Valid values | Notes |
|---|---|---|---|
| `name` | yes | any string | Title shown in the picker; merge key (project wins over global on collision) |
| `body` | yes | any string | The prompt text; may hold `{{slots}}`. Use `\n` for newlines |
| `description` | no | any string | One-line subtitle in the picker |
| `tags` | no | array of strings | Render as `#chips`; the picker fuzzy-matches them and `#tag` filters by tag |
| `placeholders` | no | array (see below) | Explicit slot defs; omit to infer plain text slots from the body |

`id`, `created_at`, and `updated_at` are populated automatically — do
**not** author them.

## Placeholder syntax

Slots in `body` are filled in by a short flow before the prompt is
injected. Two ways to author them:

- **Inline, inferred (no `placeholders` needed):**
   - `{{name}}` — a free-text slot (label defaults to `name`).
   - `{{name|formal|casual}}` — a select menu; the key is the text
     before the first `|`, the options follow.
- **Explicit `placeholders` array** — needed for labels, defaults,
  required flags, or multi-line input. An explicit def for a `key`
  wins over an inline/inferred one.

### PromptPlaceholder fields

| Field | Required | Valid values | Notes |
|---|---|---|---|
| `key` | yes | any string | Must match a `{{key}}` token in `body` |
| `label` | no | any string | Heading shown in the fill-in flow; defaults to `key` |
| `kind` | no | `"text"`, `"multi_line"`, `"select"` | Defaults to `text` |
| `default` | no | any string | For `text` / `multi_line` only |
| `options` | for select | array of strings | Required when `kind` is `select` |
| `required` | no | `true`, `false` | Block submission until filled |

```json
{ "key": "tone", "label": "Tone", "kind": "select", "options": ["formal", "casual"] }
{ "key": "notes", "label": "Extra notes", "kind": "multi_line", "default": "" }
{ "key": "ticket", "label": "Ticket ID", "kind": "text", "required": true }
```

## Scope

- **Project** (this repo only): `amf.json` — `prompt_templates`
  at the top level. Edit this file.
- **Global** (all projects): `~/.config/amf/config.json` —
  `prompt_templates` under the `"extension"` key.

Project templates appear before global ones in the picker; a global
template whose `name` collides with a project one is dropped (project
wins). If the user hasn't specified scope, default to project scope.

## Steps

1. Read `amf.json` (shown above).
2. Add the new template to `prompt_templates` (or create the array).
   Keep `body` newlines as `\n` in the JSON string.
3. Write the updated JSON back — preserve existing templates and any
   other config (presets, sessions, hooks).
4. Tell the user it will appear in the prompt library picker
   (`leader+P` inside a session, or `L` on the dashboard) as a
   read-only `Project`/`Global` entry they can inject or duplicate
   (`y`) to edit.
