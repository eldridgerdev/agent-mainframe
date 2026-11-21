+++
title = "Desktop GUI (Preview)"
description = "The work-in-progress desktop interface, and why it exists: viewing screenshots and other images."
weight = 25
+++

AMF has a second, separately usable interface: the desktop app `amf-gui`. It
is a **work in progress**. It covers only some workflows, and its features and
layout may change between releases. The terminal interface, `amf`, remains
the complete, maintained interface. Installing the GUI does not replace it.

## Why a GUI

A main reason the GUI exists is viewing images, which a terminal cannot show
well:

- **PR screenshots.** In PR Triage and PR Review, images in the PR
  description, comments and thread replies render inline, where they appear
  in the text. Click one to open a larger view with fit, original size, zoom
  and pan.
- **Agent validation screenshots.** When you ask an agent to check its work
  visually, the screenshots it captures are collected for each feature and
  session. Open **Tools → Screenshots** on a feature, or **Screenshots for this
  session** from a session tab's `⋯` menu.

The [screenshot viewer guide](https://github.com/eldridgerdev/agent-mainframe/blob/main/docs/gui-screenshots.md)
covers the details.

## Shared with the terminal interface

Both interfaces use the same projects, features and tmux sessions, so you can
switch between them freely or run both at once. You do not import or convert
anything. Use `amf` in a terminal or over SSH; use the GUI for desktop
navigation and the workflows it currently supports.

## Install

Each [GitHub release](https://github.com/eldridgerdev/agent-mainframe/releases)
includes GUI builds next to the `amf` downloads:

- **Debian/Ubuntu:** the `.deb` package, which also installs `tmux`.
- **Other Linux distributions:** the `.AppImage`. Install `tmux` yourself.
- **Apple Silicon Macs:** the `.dmg`. Builds are not yet signed with an Apple
  Developer ID, so the first launch needs approval in **System Settings →
  Privacy & Security**.
- **Windows:** run the Linux build under WSL2 with WSLg.

The agent CLIs you use must be on `PATH`. See the
[GUI installation guide](https://github.com/eldridgerdev/agent-mainframe/blob/main/gui/README.md)
for step-by-step instructions, and
[choosing an interface](https://github.com/eldridgerdev/agent-mainframe/blob/main/docs/interfaces.md)
for which workflows the GUI covers today.
