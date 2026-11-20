+++
title = "AMF Remote (Phone)"
description = "Answer, review and start agents from your phone with AMF's installable web app."
weight = 85
+++

AMF Remote lets you run AMF from your phone while the agents keep working on
your computer. It is a web app (a PWA) that AMF serves itself. There is
nothing to install from an app store, and the phone always runs the version
that matches the AMF it talks to.

<figure>
  <img src="/images/remote/showcase.png" alt="AMF Remote on a phone: the attention dashboard, a live agent session waiting on a permission question, a feature's diff, and its TODO list">
  <figcaption>The dashboard, a live session, a feature's changes, and its TODOs.</figcaption>
</figure>

From the phone you can:

- see every feature, with the ones waiting on you pinned to the top
- get a push notification when an agent needs you, and tap it to go straight
  to that agent
- read and type into any session: a readable **Simple** view with quick keys
  and a reply box, or a **Full** terminal
- start, stop, create and delete features, and add or remove sessions
- review a feature's changes against its base branch
- manage TODOs, start an agent on one, and insert prompts from your library

The phone and the desk share every session. Typing on one shows on the
other.

## Setup

1. **Start the server.** Press `Ctrl+Space C`, on the dashboard or inside any
   session. It listens on `127.0.0.1:47800`, which only this computer can
   reach.
2. **Give it an HTTPS address.** Installing the app and receiving
   notifications need HTTPS. Install [Tailscale](https://tailscale.com/download)
   on the computer and the phone, turn on MagicDNS and HTTPS Certificates in
   the admin console, then press `t` in the pairing dialog (or run
   `tailscale serve --bg 47800`). Never use `tailscale funnel`, which puts AMF
   on the public internet.
3. **Pair.** Press `Ctrl+Space Q` and scan the QR code with the phone's
   camera. The code is filled in, so tap **Pair**. Codes last five minutes
   and work once.
4. **Install.** In Chrome, use **⋮ → Add to Home screen**, open the app, and
   tap **Turn on notifications**.

<figure>
  <img src="/images/remote/01-pairing.png" alt="The AMF Remote pairing screen with numbered steps and a six-digit code" style="max-width: 320px">
  <figcaption>Pairing. Scanning the QR code fills in the code.</figcaption>
</figure>

Stuck? Press `s` in the pairing dialog for a checklist that is ticked off from
what AMF sees on this computer, or run `amf doctor`. Other tunnels
(cloudflared, ngrok) work too: point them at `127.0.0.1:47800` and set
`remote_public_url` in `config.json`.

## Security

- Each device gets its own secret token at pairing, and AMF stores only a
  hash of it. Press `v` in the pairing dialog to list devices; `d`, `d`
  revokes one immediately.
- Notifications are end-to-end encrypted to the phone and signed with AMF's
  own VAPID key. No Firebase project or AMF relay is involved.
- Anything that would stop to ask a question at the desk (a hook prompt, a
  delete with unfinished TODOs) is refused on the phone, with the reason.

<figure>
  <img src="/images/remote/03-feature.png" alt="A feature in AMF Remote showing its branch, summary, attention reason, sessions and actions" style="max-width: 320px">
  <figcaption>A feature: its sessions, actions, and why it needs you.</figcaption>
</figure>

The [full AMF Remote guide](https://github.com/eldridgerdev/agent-mainframe/blob/main/docs/remote-control.md)
covers access-control policies, non-standard Tailscale setups and every
setting.
