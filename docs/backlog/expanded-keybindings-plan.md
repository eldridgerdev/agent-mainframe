# Expanded keybindings

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** config wizard keybindings (`src/app/config_wizard.rs`,
  `src/handlers/config_wizard.rs`, `src/ui/dialogs/config_wizard.rs`);
  dashboard keybinding remaps (`src/handlers/normal.rs`); view leader
  commands (`src/handlers/view.rs`, `src/ui/pane.rs`)

## Why / problem

The config wizard can now remap a fixed list of dashboard actions, but
the rest of AMF still has hardcoded keys. The most visible gap is the
view-mode leader menu: commands such as steering, diff viewer, session
switcher, sidebar toggles, bookmarks, and remote-control helpers cannot
be customized without changing code.

Users who invest in custom workflow bindings should be able to keep one
consistent key map across dashboard actions, leader commands, and later
other modal command surfaces.

## Proposed design

- Define a broader action catalog instead of one dashboard-only list.
  Each action should have:
  - stable action id, e.g. `dashboard.refresh` or `leader.diff_viewer`
  - human label
  - default key/chord
  - scope, e.g. dashboard, view leader, dialog, picker
- Keep existing dashboard `keybindings` compatible. Either migrate old
  ids (`refresh`) to scoped ids (`dashboard.refresh`) at load time or
  continue accepting both forms.
- Extend config storage to support scoped bindings. Avoid overloading a
  single `HashMap<String, char>` if leader chords need different
  representation later.
- Update the config wizard keybindings section to filter/group by scope
  and show default versus override for every configurable action.
- Refactor leader command handling so `handle_leader_key()` and the
  rendered leader menu read from the same action catalog instead of
  separate hardcoded match arms and menu strings.

## Progress

- First view-leader bindings are now config-driven: `next_feature` and
  `prev_feature` (previously hardcoded to leader `n`/`p`) have no default,
  are read from the existing `keybindings` map, and only appear in the leader
  menu when bound. This is an ad-hoc lookup, not yet the shared scoped catalog
  below, but it establishes the "no-default, opt-in leader command" pattern.
- [ ] Inventory all hardcoded dashboard and view leader commands
- [ ] Design scoped action id + storage format
- [ ] Preserve compatibility with existing dashboard `keybindings`
- [ ] Move dashboard action defaults into the shared action catalog
- [ ] Move view leader defaults into the shared action catalog
- [ ] Update config wizard UI to browse actions by scope
- [ ] Update help/leader menu rendering from the same catalog
- [ ] Tests for remapped leader commands and compatibility migration

## Open questions

- Should leader commands be stored as single keys only, or support
  multi-key chords after the leader key?
- Should direct view-mode keys and leader-mode keys share one namespace
  or remain separate scopes?
- How should conflicts be handled when two actions in the same scope use
  the same key: warn, block save, or allow last-one-wins?
- Should custom local commands eventually be bindable through the same
  mechanism?

## Reasoning / when to build

This is worth building after the config wizard lands and the basic
dashboard keybinding picker settles. The current implementation is a
good first step, but leader commands are where power users spend most of
their time while a session is open, so making those customizable is the
next meaningful expansion.
