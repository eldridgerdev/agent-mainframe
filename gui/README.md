# AMF GUI development preview

This is the Tauri desktop interface for AMF. The `amf` terminal interface
remains available and uses the same project database and tmux sessions.

## Install a release build

Each AMF release on GitHub includes GUI builds for x86_64 and aarch64 Linux,
and for Apple Silicon Macs, next to the `amf` downloads. They carry the same version number as `amf`.
Pick the file for your machine's architecture: `x86_64-unknown-linux-gnu`
for Intel and AMD, `aarch64-unknown-linux-gnu` for ARM64.

- **Debian/Ubuntu:** download the `.deb`, for example
  `amf-gui-x86_64-unknown-linux-gnu.deb`, and install it with
  `sudo apt install ./amf-gui-x86_64-unknown-linux-gnu.deb`, which also
  installs `tmux`. Launch it from your applications menu or run `amf-gui`.
- **Other distributions:** download the `.AppImage` for your architecture, run
  `chmod +x` on it, and run it. Install `tmux` yourself.

Either way, the harness CLIs you use (`claude`, `codex`, `opencode`, `pi`) must
be on `PATH`. For a Mac, follow [macOS](#macos) below; on Windows, follow
[Windows (WSL2)](#windows-wsl2).

## macOS

Releases include `amf-gui-aarch64-apple-darwin.dmg` for Apple Silicon Macs
(M1 and later). There is no Intel Mac build; build from source on an Intel Mac.

The maintainer doesn't currently have a Mac available to set up and test Apple
Developer ID signing and notarization. CI builds the macOS releases with an
ad-hoc signature, so downloaded builds can show a **"developer cannot be
verified"** warning. Developer ID signing and notarization remain pending.

1. **Install `tmux`,** for example with `brew install tmux`, and install the
   agent CLIs you use.
2. **Open the `.dmg` and drag AMF GUI into Applications.**
3. **Approve it the first time.** The app is not signed with an Apple
   Developer ID yet, so macOS blocks the first launch with a warning that it
   can't verify the app. Close the warning, open **System Settings → Privacy &
   Security**, scroll down and click **Open Anyway** next to AMF GUI, then
   confirm. macOS remembers this, so later launches open normally.

   On a company-managed Mac, **Open Anyway** may be unavailable. See
   [Apple's explanation of these warnings](https://support.apple.com/en-us/102445)
   and the local build option below.

Apps opened from Finder, the Dock or Spotlight don't inherit your shell's
`PATH`, so at startup the GUI asks your login shell (`$SHELL`, usually zsh)
for its `PATH`. As long as `tmux` and your agent CLIs work in a new terminal
window, the GUI will find them. If your shell's startup files take longer than
three seconds, the GUI gives up and keeps the system `PATH`; launch it from a
terminal with `open -a "AMF GUI"` instead.

### Build locally without a Developer ID

You can build and run AMF on your own Mac without a Developer ID certificate or
a paid Apple Developer account. A local build normally avoids the downloaded-app
quarantine involved in that warning. Standard Git clients don't normally
quarantine their checkouts; see
[Apple's developer guidance](https://developer.apple.com/forums/thread/773965).
Company security policies can still restrict locally built programs; a source
build does not guarantee approval on a managed Mac.

Install the current stable Rust toolchain, Node.js **22.12 or later**, `tmux`,
and the agent CLIs you use. For the compiler and macOS SDK, Xcode Command Line
Tools are sufficient; the full Xcode app is not required for this desktop build.
See [Tauri's macOS prerequisites](https://v2.tauri.app/start/prerequisites/#macos).
If the command line tools aren't installed, run:

```sh
xcode-select --install
```

Clone AMF with Git onto the Mac. From the repository root, build and launch a
standalone executable:

```sh
cd gui
npm ci
npm run tauri -- build --no-bundle --ci
../target/release/amf-gui
```

The executable is `target/release/amf-gui` at the repository root. Run it again
from a terminal whenever you want to open the GUI; `tmux` and your agent CLIs
must remain on `PATH`.

For development with automatic rebuilding, run this from `gui/` after
`npm ci` instead:

```sh
npm run tauri dev
```

## Windows (WSL2)

There is no native Windows build. AMF runs every agent session in tmux, and
tmux doesn't run on Windows, so the GUI runs as a Linux app inside WSL2 and
opens as a normal window through WSLg.

You need Windows 11, or Windows 10 version 21H2 or later with the Microsoft
Store version of WSL.

1. **Install WSL2 with Ubuntu.** In PowerShell as administrator, run:

   ```powershell
   wsl --install -d Ubuntu
   ```

   Restart when asked, then open **Ubuntu** from the Start menu and create your
   Linux user. If WSL was already installed, run `wsl --update` instead to get
   a version with WSLg.

2. **Download the GUI inside Ubuntu.** Use the Ubuntu terminal, not Windows,
   so the `.deb` ends up in the Linux file system. On an ARM PC, use
   `aarch64-unknown-linux-gnu` in place of `x86_64-unknown-linux-gnu`.

   ```sh
   curl -LO https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-gui-x86_64-unknown-linux-gnu.deb
   ```

3. **Install it.** This also installs `tmux` and the GUI's runtime libraries:

   ```sh
   sudo apt update
   sudo apt install ./amf-gui-x86_64-unknown-linux-gnu.deb
   ```

4. **Install your agent CLIs inside Ubuntu.** Install `claude`, `codex`,
   `opencode` or `pi` in Ubuntu the same way as on Linux. A CLI installed on the
   Windows side isn't visible to the GUI.

5. **Keep your repositories in the Linux file system,** for example under
   `~/code`, rather than under `/mnt/c`. Git and agents are much slower across
   the Windows boundary, and worktrees are created next to the repository.

6. **Start the GUI.** Run `amf-gui` in the Ubuntu terminal. WSLg usually also
   adds an **AMF GUI** entry to the Windows Start menu, marked with your distro's
   name.

The GUI and `amf` share one database inside WSL (`~/.config/amf/amf.db`), so if
you also use `amf` there, both see the same projects and sessions. The GUI's
terminal includes the icon fonts agent status lines use, so you don't need a
Nerd Font installed in WSL.

## Run from source

Install the current stable Rust toolchain, Node.js 22.12 or later, a C compiler,
`tmux`, and the harness CLIs you intend to use. On Linux, install the system
libraries in [Tauri's Linux prerequisites](https://v2.tauri.app/start/prerequisites/).
On macOS, see [Build locally without a Developer ID](#build-locally-without-a-developer-id)
for prerequisites and a standalone build that doesn't need a signing account.
Run the app from a shell with `tmux` and the harness CLIs on `PATH`:

```sh
cd gui
npm ci
npm run tauri dev
```

For a standalone release-mode binary without an installer, run
`npm run tauri -- build --no-bundle --ci`. The executable is
`target/release/amf-gui` at the repository root. This build still needs
`tmux` and the harness CLIs available at runtime.

On Windows, build from source inside WSL2 the same way, after step 1 of
[Windows (WSL2)](#windows-wsl2).

## Current coverage

The [workflow inventory](../docs/backlog/amf-gui-workflow-inventory.md) labels
each available, limited, and planned GUI workflow. The GUI currently supports
project and feature creation, additional Claude, Codex, OpenCode, Pi, terminal,
and Neovim sessions, session terminals, TODO lists and agent starts,
Full and Quick Plan interviews, and Learning with persisted Q&A and an explicit
editing-agent handoff, plus standalone Git diffs, supervised edits, saved prompt browsing and
dormant-feature stops. Continue to use `amf` for workflows
marked Planned.

Both interfaces read the existing `~/.config/amf/amf.db`. The GUI checks for
external workspace and TODO changes every two seconds. Each feature page has
one tab per session; leaving a session's tab detaches the GUI's view and leaves
the tmux agent session running, while Stop ends the feature session. Agent
starts that hit AMF's resource warning ask for explicit approval.

The sidebar is the TUI dashboard tree. Each project shows its shortened path,
an add-feature hint when it is empty, and any minimized creation-time plan.
Each feature row shows the TUI's status glyph (worktree script, deletion in
this window, waiting for input, agent working, ready, then running/idle/
stopped), its nickname and branch, then badges: `repo`, issue source, PR state
and open threads, token usage, mode, `review`, `plan`, `plan paused · Resume`,
`remote`, age, session count, stopped sessions, the `?` request marker and the
AI summary. The workdir and full text are in tooltips. Expanding a feature lists
its sessions with kind icon, running state, the agent context indicator and
the status line; selecting a row opens that tab. Collapse state is the
`collapsed` flag the TUI uses, so it round-trips between interfaces. The GUI
derives context, usage, status text and open PRs with the TUI's own background
collectors (the PR sweep reads GitHub through `gh` every five minutes), and reads
merged/closed PRs, the hooks' thinking markers and waiting notification files
from their shared on-disk forms. The attention reason, a TUI background hook,
TUI deletion, summary generation and AI-review progress exist only inside a
TUI process and are not shown. While a TUI is running, hooks report to it
instead of the shared files, so the GUI can under-report thinking and waiting.

Agent tabs include **Compose prompt** below the terminal. Type or paste locally;
**Enter** adds a line, and **Ctrl/Cmd+Enter** or **Send prompt** sends the whole
message through tmux's bracketed-paste path. Each session keeps its own unsent
draft while this GUI window stays open, including when you switch tabs or leave
the feature page. Failed sends keep the draft for retry. **Clear** clears only
the current session's draft. TODO, planning and Learning handoffs fill this same
composer, appending to any existing draft. Click the terminal whenever you need
to interact directly with the harness; shell and editor tabs use direct input.
Stopped agent tabs also show their editable draft, with sending disabled until
that session starts and its terminal connects.

Open **Prompt library** in workspace navigation or an agent composer. The current project or feature determines the initial library scope;
you can switch to another scope or just user/global templates. Search matches
names and bodies, with `#tag` filtering. Source badges distinguish user, global,
project and worktree copies. Select a template, fill its text/multiline fields
and configured or inline choices, then review the resolved preview. **Add to
draft** opens the chosen allowed agent session and appends to its unsent text;
it does not start an agent or send a message. Templates and target permissions
are rechecked on insertion. The list refreshes while you browse, and a
selected template edited or deleted elsewhere is deselected with a notice so
you never fill an outdated copy. The preview renders as you type. Cancel
leaves existing composer drafts intact. Create/edit/delete/export templates and
editable prompt overrides have their own manager (below). Native desktop
interaction and macOS runtime validation for this library increment remain open.

Open **Prompt overrides** in workspace navigation to change the prompts AMF
sends for headless AI calls (plan interviews, Final Review walkthroughs and
questions, AI PR review, review memory, Learning answers and session
summaries). It is separate from the prompt library: library templates are text
you insert into an agent draft, while overrides replace AMF's own built-in
prompts. Choose a context (global, a project, or a feature) and the harness to
preview. Each prompt shows the layer in effect, its placeholders, effective
template, built-in default and every stored override. **New override…** or
**Edit** opens a local editor; choose **This feature** (stored in AMF's
database for that checkout), **This project (amf.json)** (committed with the
repo) or **Global**, and a shared or harness-specific template, then **Save
override**. **Clear…** removes exactly one stored override after confirmation.
Escape, switching prompts or contexts ask before discarding unsaved text. The
list refreshes while open. If the TUI, another window or a hand edit changes
that prompt's overrides, saving is blocked until you choose **Reload current
version**, which keeps your text. A malformed `amf.json` is reported and never
overwritten. A pending Final Review or planning AI call offers **Edit prompt**,
which opens the manager on that prompt. The call keeps waiting and uses your
saved override when you continue, though its preview still shows the earlier
text. Native macOS validation remains open.

Use **New session** on a feature page to start another agent, terminal, or
Neovim session. You can name it or use the next default name; the new tab opens
when creation succeeds. The picker shows the agents allowed for that project.
If tmux exits unexpectedly, the GUI shows the affected features as stopped.
Start a feature to recreate its tmux session; when a saved Claude, Codex, or
OpenCode session is available, the GUI offers to resume it, start fresh, or
choose another saved session.

**Delete feature** asks what to do with unfinished worktree TODOs. If the feature
also hosts the project TODO list, choose a surviving feature to keep that list
or explicitly delete the list and all its TODOs. Both choices are collected
before deletion starts; **Cancel** leaves the feature and lists untouched.
The first surviving feature is selected by default. Deleting the last feature
drops a project list hosted by it, matching the TUI.

When a project's worktree setup asks for an option, **New feature** and the
TODO's new-feature form show that choice before creation. Choose an option to
continue; a failed lookup offers **Retry**. The configured script receives the
choice through `AMF_HOOK_CHOICE`. This works with direct starts, Full and Quick
Plan, and TODO planning. Setup runs before the interview opens; cancelling the
interview keeps the worktree and its setup changes, without saving a plan or
launching an agent. A failed setup still allows planning to continue and shows
a notice.

Use **Learning** on a feature page to browse its files or branch changes and
ask about the project, a file, or a selected range (click, then Shift-click).
Choose an answering harness and reading level; answers and follow-ups share
the TUI's Learning history. **Deep dive** lets the answering agent read the
repository. Codex always uses its read-only repository sandbox. Reading and
asking questions do not open an editing session. **Open editing agent** starts
an agent only when explicitly selected, asks for resource approval when needed,
and opens an editable prompt that you can review before sending. Closing
Learning leaves pending answers running while AMF stays open. Unsent questions
are preserved across refreshes and require explicit discard on close.

Learning also offers starter questions, hunk selection, keeping an answer as an
editable TODO, intent relabelling and restarting a stopped linked editing session.
Learning command and component tests use mocked harness execution; native desktop
Learning interactions and paid-harness runs have not been validated yet.

Use **Changes** on a Git feature to browse all current changes (committed,
staged, unstaged and untracked) or one commit from the feature's history.
The viewer works while the feature is stopped. Filter files, jump between hunks,
switch unified/side-by-side layouts, ignore whitespace, expand context, or
choose a different base ref for current changes. **Refresh** reloads Git's
current contents. Native WSLg captures verify opening from a stopped feature, switching layouts
and context, selecting a commit and displaying a binary notice through real
Rust IPC. Other native interactions and macOS remain unvalidated. Command
tests use isolated Git repositories; component tests mock the IPC calls.

Use **Final Review** on a Git feature to approve, reject or skip files, undo a
verdict, write whole-file or line/range comments with severity, resolve/reopen
comments, edit suggested replacements, and save overall feedback. Click a line
number to select it; Shift-click another to extend a range in diff order. Use
**Comment on selection** or **Suggest replacement**, or edit a saved thread.
Empty saves clear prose or replacement code while keeping the other part of
the thread. Authoring suggestions saves them for later feedback. **Apply suggestion
locally** offers an explicit confirmation before writing a kept, unresolved
replacement to the checkout. It checks the unchanged reviewed file and the exact
current-side span; deletion-side ranges, lost anchors and AI drafts cannot be
applied. Success resolves the thread, consumes its replacement, refreshes the
diff and clears the changed file's approval and verdict undo. **Applied locally**
lists the persisted application history. A failed source write keeps the suggestion;
a failed progress save retains the already-applied source change and offers
**Retry save**. Closing without saving does not undo source changes.
Lost anchors stay visible and require refresh before editing. Developer notes and saved line threads/suggestions are
shown alongside unified or side-by-side diffs. **Pause review** preserves progress
in the same format the TUI resumes; reopening either interface continues it.
Unsaved form edits require explicit discard before leaving, and failed saves
retain edits with **Retry save**, or **Close without saving** when the save
cannot succeed. **Refresh changes** reloads the diff and clears
approvals and verdict-undo entries for changed patches, and reopening a paused
review drops approvals whose patch changed meanwhile; **Reload
saved review** adopts progress changed in another interface. Edit a given review
in one interface at a time; detected external saves block edits until reload.
**Generate walkthrough**, **Changeset overview** and **AI co-review file** reuse
AMF's Claude review tools. Each new call offers a prompt preview and requires
**Continue AI call**; cancelling runs nothing. Walkthroughs are cached for files
without developer notes, and co-review findings stay AI drafts until **Accept AI
draft** or **Dismiss AI draft**. Accepting a draft uses the same verdict rules
and saved progress as a human line comment.

**Ask about file** or **Ask about selection** opens a local question draft.
Choose an allowed Claude, Codex, OpenCode or Pi harness, then ask explicitly.
The answering harness reads the repository and reviewed diff; follow-ups include
earlier answers for the current review version. Questions, answers and AI notes
stay in memory while this review is open; pause/reopen retains verdicts/comments,
including accepted co-review findings, but starts a new conversation. Failed
questions can be retried, cancelled pre-call notices retain question text, and
closing an unsent question requires explicit discard. Polling never starts a new
AI call. Changed patches, moved/deleted feature targets or external progress edits
discard pending AI results and require refresh/reload before continuing.
**Cancel AI request** releases question and single-process runs; an already-started
batched co-review may finish its current work, but its late findings are discarded.

From an answered question, choose **Draft inline comment** (for questions about
selected lines) or **Draft overall feedback**. The answering harness drafts the
feedback after another prompt preview and confirmation. Edit **AI comment draft**,
then choose **Open comment editor** to check the repository again without another
AI call. Transfer appends to existing feedback, preserves a containing thread's
full range and severity, and refuses ambiguous overlaps. Nothing is saved until
**Save comment** or **Save overall feedback**. Failed transfers retain your edits;
leaving a generated or transferred draft requires explicit discard.

**Review history** opens a read-only round browser. **Current** shows this open
review's verdicts, feedback, resolved threads, AI drafts and prior agent replies;
unsaved form text stays in its editor. Completed rounds appear newest first with
their original feedback, suggestions, check output and agent replies.
**Load older rounds** reads archived history on demand. **Return to review**
or Escape closes the browser and keeps local drafts. Browsing saves no progress
and leaves outstanding save errors available for retry after returning.
Seven asserted native WSLg frames verify Current/completed rounds, archived
history, restored draft text, archive read failures and retry after repair through
real Rust IPC. Source and progress assertions confirm browsing writes neither.
Reproduce with `scripts/dev/screenshot/scenarios/gui-review-history.txt`. Remaining
interactions and macOS validation stay open.

**Pre-finish summary** shows every file verdict, open kept thread and overall
feedback, including files hidden by your filter. Browsing keeps unsaved drafts
and writes no progress. **Apply pending suggestions** requires a separate
confirmation before writing source files through the same batch engine as TUI
finishing. Successful replacements settle their threads, refresh the diff and
clear changed-file approvals; blocked jobs stay open with an explanation. The
review stays open so you can inspect and review changed code again. Failed
progress saves keep the source changes and require **Retry save** before another
application; closing without saving does not undo those source writes.

**Complete review…** in the summary opens a confirmation that lists what the
round records: verdict and comment counts, files without a verdict (recorded as
skipped), any saved apply-on-finish batch, the configured check and PR posting.
Completing works like the TUI finish. It applies that batch, reruns the
configured check (earlier results in the summary are not reused) and records
the round in `.claude/final-review-feedback.md`. It then clears saved progress
and closes the review. While the check runs you can **Cancel completion**, and
nothing is written. Choose **Complete and hand off to …** to give an actionable
round's "address the feedback" prompt to the feature's first agent session. It
is submitted when the TUI's `final_review_submit_prompt` setting is on and the
session is running. Otherwise it is added to that session's composer as an
unsent draft. **Complete without handoff** only saves the feedback. Unsaved or
generated drafts, failed saves, changed patches or saved progress, a changed
check command or agent session, and deleted or moved features are refused
before anything is written. The completion is delivered once. Choosing another
destination (a dedicated session, another feature or a companion feature)
remains in the TUI.

The summary, batch and completion controls have automated command/component
coverage; native desktop and macOS interaction validation remain open.

Question-to-comment drafting has command/component coverage and eight asserted
native WSLg frames from `gui-review-question-drafts.txt`, including edited drafts,
confirmation/cancellation, inline and overall transfer, unsaved protection and
pause/reopen persistence. The capture uses offline Codex and real Rust IPC;
paid-harness execution, other native interactions and macOS validation remain open. Automated
command and component tests cover the new AI controls. Eight asserted native WSLg
frames from `gui-review-ai.txt` also verify prompt preview, walkthrough/overview
output, co-review drafts, local multiline questions and answers, accepted-finding
persistence and changed-patch refusal through real Rust IPC. The isolated scenario
uses offline CLI fixtures; paid-harness runs remain unvalidated. Native WSLg captures verify
opening on a stopped feature, notes and saved line threads, verdict/comment persistence, split layout, unsaved-draft protection,
pause/reopen, changed-patch rejection and approval invalidation after refresh.
The isolated `gui-review-line-comments.txt` scenario also verifies range
selection, line/range comment and replacement editing, split display,
unsaved-suggestion protection, resolution/reopening and restored thread anchors.
The `gui-review-local-suggestions.txt` scenario verifies confirmed local application
and cancellation, source writes and approval invalidation, stale/read-only source
refusals, progress-save retry and restored application history through real Rust IPC.
Remaining native interactions and macOS validation are open.

### Supervised edits

A Vibeless agent's hook holds each file change until AMF answers it. When no
AMF TUI is running, the hook leaves the request on disk and the GUI picks it up:
the feature gets a count in navigation, a notice offers **Review**, and the
feature page shows **Supervised edits** while any edit waits (a Vibeless
feature's **⋯** menu always has it). The panel lists waiting edits with the
hook's captured diff, the agent's stated reason when it gives one, and layout
and context controls. **Approve edit**, **Reject edit** (with optional
feedback, up to 200 characters) and **Cancel edit** each need a second,
explicit confirmation that states what the agent will do; OpenCode, for
example, does not forward rejection feedback and treats cancel as a skip.
Answers name the reviewed revision, so an edit that changed, was answered in
another window, or whose agent stopped waiting is refused rather than answered
twice. Closing with unsent feedback asks before discarding it. AMF never writes
the source itself; the agent does, after reading an approval.

A running TUI receives these requests over its socket and answers them there;
the GUI does not take that socket over. Explaining an edit with AI also stays
in the TUI. `gui-supervised-edits.txt` drives the real Claude hook script with
offline input against an isolated database and checkout.

### Dormant features

Open **Dormant features** in workspace navigation for the TUI's `z` list:
running features whose agent has produced no output for longer than
`dormant_idle_minutes` *and* that nobody has opened for longer than
`dormant_last_accessed_hours`, longest idle first, with each age and timestamp.
Setting either key to `0` switches detection off. Select features and confirm
**Stop** to stop them through the TUI's stop, including editor cleanup when
`kill_editor_on_stop` is on. Each feature is checked again before stopping, and
the result lists what happened to each one: which editor windows were closed,
which were left running and why, and which features were skipped because they
changed. Showing a session's terminal counts as opening the feature, as in the
TUI. Closing only an editor and deleting from this list remain TUI actions.
Four asserted native WSLg frames from `gui-dormancy.txt` verify the list,
confirmation, a confirm-time refusal and editor ownership with stand-in
processes on a private tmux server.

## Checks

```sh
cd gui
npm test
npm run build
cd ..
cargo test --workspace --locked
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The release workflow (`.github/workflows/release.yml`) builds the `.deb` and
AppImage for x86_64 and aarch64 Linux and the `.dmg` for Apple Silicon from
each version tag. If a GUI build fails, the `amf` release is still published.
The macOS build is ad-hoc signed, not signed with a Developer ID or notarized.
Developer ID signing, notarization, Intel Mac builds and in-app updates remain
packaging work.
