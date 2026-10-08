# Viewing screenshots in the GUI

PR Triage and PR Review show images inline in the **PR description**, selected
comment and thread replies. Markdown images, reference images and GitHub's HTML
`<img>` upload markup render in their original position. Click an image to
open a larger view with fit, original size, zoom and pan. Closing it preserves
your selected comment and unsent reply. Failed images have an inline retry.

PR links open in your browser. Reading a comment does not build a separate
image gallery or automatically download Actions artifacts or linked galleries.
The description has its own retry if GitHub cannot be read.

Use **Screenshots** on a feature to review agent validation evidence, or
**Session screenshots** to filter to the selected producing session. These
entries also work for stopped features and ordinary non-git directories.
The workspace's **Validation screenshots** entry includes historical evidence
after the original sessions/features disappear.

Click a thumbnail to open the shared viewer. Use **Fit**, **Original size**,
**Zoom in/out**, and drag or scroll to pan. **Previous/Next** or the arrow keys
move between images. Escape returns to the gallery, then closes the gallery;
your PR comment selection and unsent reply stay in place. **Retry image** retries
a failed remote read where it appears.

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
and HTML galleries do not render in the viewer. GitHub's HTML image markup is
supported when it references a supported image. Gallery links open in your
browser using its authentication. Private attachments are retrieved as images.

GitHub CLI authentication is reused for GitHub sources. If an uploaded image
returns an HTML page or fails to download, AMF asks GitHub's authenticated API
for a fresh image URL and retries automatically. Private attachment and
artifact support is implemented but **live authenticated private-source
validation is still pending**. GitHub Enterprise hosts are currently unsupported.
Images, archives and history are bounded; source errors and limit notices stay
visible. See [coverage and acceptance](development/gui-screenshot-viewer.md) for
verified cases and the native check command.
