+++
title = "Project Config Reference"
description = "What a project's amf.json can declare, and how it merges with global config."
weight = 92
+++

A project's `amf.json` (repo root — see [Configuration](@/docs/configuration.md))
is meant to be committed: it's how a team shares presets, sessions, hooks,
and plan-interview questions across everyone working in the repo, layered
on top of each person's own global `~/.config/amf/config.json`. This page
is the field reference for what it can hold.

| Key | Holds | Edit via | Merge with global |
| --- | --- | --- | --- |
| `custom_sessions` | Dev servers or other persistent commands offered in the `s` session picker | Config wizard | Project appends; a name collision lets the project entry win |
| `feature_presets` | Named create-feature templates (agent, mode, branch prefix, plan mode, review, Chrome) | Config wizard | Same append/override-by-name rule |
| `lifecycle_hooks` | `on_start` / `on_stop` / `on_worktree_created` scripts, optionally prompting the user with options first | Config wizard | Each of the three hooks independently overrides its global counterpart when set |
| `keybindings` | Remap a dashboard action to a different key | Config wizard | Project overrides global per action |
| `allowed_agents` | Restrict which harnesses this project offers | Config wizard | Project replaces the whole list when set |
| `plan_questions` | Extra plan-interview questions layered onto the built-in bank | Config wizard | Merged by stable ID: project wins on collision, appends otherwise |
| `skip_builtin_questions` | Drop AMF's built-in plan questions, keeping only configured ones | Hand-edit | Project overrides global when explicitly set |
| `prompt_templates` | Reusable [prompt-library](@/docs/prompts-and-todos.md#reuse-prompts) templates | Prompt library (`L`) export | Merged by name: project wins on collision |
| `prompt_overrides` | Project-scope [headless AI prompt overrides](@/docs/custom-prompts.md) | Prompt-override manager (`E`) | Project-only — never inherited from global |
| `final_review_check_command` | A build/test command run when finishing a [final review](@/docs/review-and-pr-feedback.md) | Hand-edit | Project overrides global when set |
| `review_memory_path` | Path to the [review-findings memory doc](@/docs/review-and-pr-feedback.md) | Hand-edit | Project overrides global when set |
| `review_prompt_budget_tokens` | Token ceiling before [batched review](@/docs/review-and-pr-feedback.md) kicks in | Hand-edit | Project overrides global when set |

The config wizard (`Ctrl+Space`, then `c`) covers everything marked
"Config wizard" above — see
[Configuration](@/docs/configuration.md#built-in-customization-skills) for
how to open it and for the `amf:configure` family of skills that can edit
these on your behalf. Everything marked "Hand-edit" has no wizard step yet;
edit `amf.json` directly.

A field with no entry in the project's `amf.json` simply inherits the
global value — an empty project config never overrides anything.
