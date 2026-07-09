# Docker No-Tmux Image

This image is for testing AMF installation in a container that does not
preinstall the system `tmux` package.

It installs the latest AMF release bundle into `/opt/amf`, installs
Claude Code into the image, then exposes wrapper commands in
`/opt/amf/bin` that point at the bundled `amf` and `tmux` binaries
from that release.

For local testing of a checkout, use
[`scripts/dev/package-no-tmux-test-bundle.sh`](../scripts/dev/package-no-tmux-test-bundle.sh)
to create a runnable archive from the current `amf` binary plus the
existing bundled `tmux` installation. That avoids glibc mismatches
when running the bundle inside the Debian-based no-tmux image.

## Build

```bash
docker build -f docker/no-tmux/Dockerfile -t amf-no-tmux .
```

## Run

```bash
scripts/dev/run-no-tmux-docker.sh -- bash
```

The container starts with a shell after installing AMF. Inside it, you
can verify that the base image did not ship `tmux`, that `claude`
is available, and that AMF works from the bundled release:

```bash
command -v tmux || true
command -v claude
claude --version
amf -V
tmux -V
```

## Custom Release

You can point the installer at a different archive without changing the
image:

```bash
AMF_RELEASE_ARCHIVE="$PWD/amf.tar.gz" scripts/dev/run-no-tmux-docker.sh
```

To test the current checkout, build a local bundle first:

```bash
cargo build --release
scripts/dev/package-no-tmux-test-bundle.sh target/release/amf /tmp/amf-no-tmux-test.tar.gz
AMF_RELEASE_ARCHIVE=/tmp/amf-no-tmux-test.tar.gz scripts/dev/run-no-tmux-docker.sh -- bash
```

Or override the download base URL:

```bash
AMF_RELEASE_BASE=https://github.com/eldridgerdev/agent-mainframe/releases/latest/download \
  scripts/dev/run-no-tmux-docker.sh
```

Set `AMF_SKIP_INSTALL=1` if you want the container to start without
running the installer.
