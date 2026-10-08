# Viewing screenshots in the GUI

Open **Screenshots** in PR Triage to review images from the PR description,
conversation comments, review comments/replies and summaries, repository files,
linked image manifests and GitHub Actions artifacts. Repository images show the
resolved commit. Actions images show the run, artifact and archive member.
Attachments show where they were discovered; that does not establish the commit
from which an attachment was captured.

The default Actions selection is the latest completed run at the PR's current
head with unexpired supported images, across workflows and conclusions. Select
an older run or use **Older runs page** to browse earlier evidence. Reruns use
their latest start time. GitHub does not identify a producing attempt for every
artifact, so the viewer labels that uncertainty explicitly.

Use **Screenshots** on a feature to review agent validation evidence, or
**Session screenshots** to filter to the selected producing session. These
entries also work for stopped features and ordinary non-git directories.
The workspace's **Validation screenshots** entry includes historical evidence
after the original sessions/features disappear.

Click a thumbnail to open the shared viewer. Use **Fit**, **Original size**,
**Zoom in/out**, and drag or scroll to pan. **Previous/Next** or the arrow keys
move between images. Escape returns to the gallery, then closes the gallery;
your PR comment selection and unsent reply stay in place. **Retry image** or
**Refresh sources** retries a failed remote read.

New Claude and Codex launches receive a producing-session destination and
completion instructions. Those instructions permit capture only when you
explicitly request visual validation. Other harnesses do not receive this
initial guidance. Existing running sessions need a new launch to receive it.
Images require a completion manifest with ownership and a hash; unfinished
images remain in the gallery's incomplete-evidence diagnostics. The exact
formats and publication steps are in the
[evidence contract](development/screenshot-evidence-contract.md).

Worktree deletion removes its screenshot files. Failed deletion retains the
evidence. Repository-root and non-git screenshots remain until you choose
**Screenshot cleanup…**, select a producing scope, and confirm **Delete
screenshots**. Cleanup also remains accessible after the feature is gone. A
partial removal can be retried. Restart the producing agent after cleanup to
receive a fresh destination.

PNG, JPEG, WebP and GIF are supported; animations show their first frame. SVG
and HTML galleries do not render in the viewer. Public galleries need an
explicit image-manifest link. Protected and unsupported public galleries offer
browser opening using your browser's authentication. Private attachments remain
image retrievals and cannot silently fall back to browser-only coverage.

GitHub CLI authentication is reused for GitHub sources. Private attachment and
artifact support is implemented but **live authenticated private-source
validation is still pending**. GitHub Enterprise hosts are currently unsupported.
Images, archives and history are bounded; source errors and limit notices stay
visible. See [coverage and acceptance](development/gui-screenshot-viewer.md) for
verified cases and the native check command.
