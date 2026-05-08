---
name: amf:release-notes
description: >
  Update CHANGELOG.md for AMF releases. Use when writing or revising
  release notes and keep the notes focused on what AMF users will notice:
  behavior changes, workflow impact, new capabilities, fixes, defaults,
  compatibility, and migration steps. Avoid implementation details unless
  they explain user impact.
allowed-tools: Bash(cat *) Bash(test *) Edit(CHANGELOG.md)
---

# Release Changelog Guidance

## Audience

Write for the person using AMF, not for the engineer reading the code.
Assume they want to know:

- what changed in how they use the tool
- whether a release affects their workflow or setup
- whether they need to change config, restart, migrate, or re-learn a step

## What to emphasize

- user-visible behavior
- new features and workflow changes
- fixes that remove a problem users would notice
- default changes and compatibility changes
- migration or action required after upgrading

## What to avoid

- internal implementation details
- refactor notes
- file names, module names, and code-path trivia
- low-level debugging or architectural commentary unless it explains user impact

If an implementation detail matters, reduce it to one short clause and
immediately explain what it means for the user.

## Writing rules

- Lead with the practical effect
- Prefer plain language over technical jargon
- Keep bullets short and specific
- Answer "so what?" for every item
- Group notes by user impact, not by subsystem, unless the subsystem is what the user experiences
- Include a `Migration` section when users may need to do something after upgrading
- Say "No migration is required" when appropriate

## Good shape

- "AMF now keeps worktree-specific hooks isolated, so changing one branch does not affect another."
- "Fixed startup failures on macOS, so AMF opens reliably again."
- "Changed the default session picker behavior, so new features now start with a clearer review flow."

## Bad shape

- "Refactored hook installation to use local settings files."
- "Adjusted tmux client spawning logic."
- "Cleaned up release plumbing and persistence internals."

## When updating CHANGELOG.md

- keep the Keep a Changelog structure
- keep `## [Unreleased]` current
- use release sections like `Added`, `Changed`, `Fixed`, `Removed`, `Security`, and `Migration` only when they help readers
- remove filler and implementation noise
- make each release section understandable on its own

