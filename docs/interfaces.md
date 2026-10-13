# Choose an AMF interface

AMF has two separately usable interfaces. The terminal interface remains
maintained alongside the desktop GUI; installing the GUI does not replace it.

| | Terminal: `amf` | Desktop: `amf-gui` |
| --- | --- | --- |
| Use it from | A terminal, including an SSH session | A local desktop window |
| Platforms | Linux and macOS; Windows through WSL | Linux and Apple Silicon macOS release bundles; Windows through WSL2/WSLg |
| Workflow coverage | The complete AMF workflow surface | Staged coverage, with available and deferred workflows listed in the [inventory](backlog/amf-gui-workflow-inventory.md) |
| Install | [TUI release bundle, crates.io or source](../README.md#installation) | [GUI release package or source](../gui/README.md#install-a-release-build) |

The GUI is a development preview. Its release bundles are separate downloads
beside the TUI bundles. There is no native Windows GUI release or native
Windows session backend. macOS bundles are ad-hoc signed and unnotarized;
see the [macOS instructions](../gui/README.md#macos) for current limitations.

## Shared projects and sessions

For the same operating-system user and AMF configuration location, both
interfaces use the same SQLite store, `amf.db` in the AMF config directory, and
the same tmux sessions. That directory is normally `~/.config/amf` on Linux and
WSL (`$XDG_CONFIG_HOME/amf` when that variable is set) and
`~/Library/Application Support/amf` on a fresh macOS install. An earlier macOS
install that already has data in `~/.config/amf` keeps using it. You do not
import or convert a workspace when switching interfaces. A GUI running inside
WSL shares with the TUI inside that Linux distribution, rather than a separate
user or remote machine.

The GUI checks the database for external workspace and TODO changes every two
seconds. Viewing a session in either interface attaches another view of the
same session. Leaving a GUI tab or closing the GUI detaches its view; it does
not stop the agent. Explicit Stop and Delete actions control session and
feature lifetimes.

Workspace saves detect stale snapshots instead of overwriting another
process's committed changes. If an action reports a conflict, refresh and
review the current target before retrying. Keep an interview, review or
configuration edit in one interface at a time: unsaved drafts and active
workflow state are not a shared live editor. The GUI's prompt composer drafts
belong to its open window. Config writers outside the desktop settings dialogs
do not all participate in their scoped-write lock.

GUI navigation preferences, such as sidebar visibility and a GUI theme
override, are local to that viewer. Shared feature collapse state and session
bookmarks still round-trip through the store. The default GUI theme follows
the TUI theme until a GUI override is selected.

## What the GUI currently covers

The [GUI guide](../gui/README.md#current-coverage) explains the available flows;
the [workflow inventory](backlog/amf-gui-workflow-inventory.md) records their
limits and validation evidence. The [release-readiness evidence report](development/gui-release-readiness.md)
separates the audited main baseline, unmerged review queue and remaining
platform/release gates. Source capabilities can be newer than the
last published release, so check the release's notes and tagged documentation
when installing a bundle.

Current source coverage includes project and feature lifecycle actions,
agent and other session types, live terminals and prompt composition, scoped
TODOs, Full and Quick Plan, Learning, diffs, Final Review (comments and
suggestions plus AI walkthroughs, the changeset overview, co-review drafts and
repository-aware questions), supervised edit approval, PR Triage and project
PR Review, prompt browsing and overrides, themes, dormancy and session
bookmarks.

Deferred GUI capabilities include broader configuration and harness setup,
prompt-template authoring/export, advanced PR Review authoring, and additional
PR Triage and feedback destinations. Use the TUI for these workflows. Native
macOS interaction and installed-package upgrade/session-continuity checks are
still open; hosted Rust tests do not establish those desktop behaviors.

## Upgrade the interfaces independently

Prefer GUI and TUI builds from the same AMF release. Shared persistence does
not guarantee that arbitrary older and newer versions work together. Startup
can apply database and configuration migrations; a downgrade is not an
automatic reverse migration. Keep a backup before switching release lines.

- `amf upgrade` updates the TUI release bundle; it does not update the GUI.
- Update a Debian/Ubuntu GUI installation by installing the newer `.deb`
  with the same package name, `amf-gui`.
- Replace an AppImage with the newer architecture-matched AppImage.
- Replace the macOS application using the newer `.dmg`.
- Source installations rebuild the executable for the interface being used.

Close the GUI and exit the TUI before replacing their executables, then reopen
them on the existing store. Exiting a viewer leaves agent tmux sessions
running. Avoid a feature Stop or Delete if the intention is only to update the
viewer. No GUI in-app updater is currently provided.

Installing or updating one interface does not require uninstalling the other.
The [development checks](development/checks.md) cover the Rust workspace
suite, and the [GUI checks](../gui/README.md#checks) add the frontend test and
build steps and the release-packaging notes. The
[packaging workflow](../.github/workflows/release.yml) builds both sets of
release bundles.
