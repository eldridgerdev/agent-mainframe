# AMF Remote (phone companion)

AMF Remote lets you run AMF from your phone while the agents keep working on
your computer. It is a web app (a PWA) that AMF serves itself: there is nothing
to install from an app store, and the phone always runs the version that
matches the AMF it talks to.

From the phone you can:

- see every feature, and which ones need you (the same list as the desk's `i`
  view)
- get a push notification when an agent needs you, and tap it to go straight
  to that agent
- open any session's terminal, read it (with scrollback), and type into it —
  a readable **Simple** view with quick keys and a reply box, or a **Full**
  terminal (xterm.js)
- start and stop features, add and remove sessions, create and delete
  features
- review a feature's changes against its base branch
- manage TODOs (worktree, project and global lists), and start an agent on one
- insert prompts from your prompt library, with their `{{slots}}` filled in

The phone and the desk share every session: typing on one shows on the other,
and the last keystroke wins, just like two terminals attached to one tmux
session.

## Setup

### 1. Start the server

Press `Ctrl+Space C` — on the dashboard or inside any session. The server
starts only when you ask for it and stops when you press it again or quit AMF.

By default it listens on `127.0.0.1:47800` — this computer only. To reach it
from a phone, put it behind a tunnel (next step).

### 2. Give it an HTTPS address

Installing the app and receiving notifications need HTTPS. The simplest
option is [Tailscale](https://tailscale.com) on both the computer and the
phone:

```sh
tailscale serve --bg 47800
```

This prints an address like `https://my-pc.tailnet-name.ts.net`, reachable
only from devices on your tailnet. (The first time, Tailscale may ask you to
enable HTTPS certificates and Serve for your tailnet in its admin console.)

Then tell AMF that address, so the pairing QR points the phone at it. In
`~/.config/amf/config.json`:

```json
{
  "remote_public_url": "https://my-pc.tailnet-name.ts.net"
}
```

Other tunnels (cloudflared, ngrok) work the same way: point them at
`127.0.0.1:47800` and set `remote_public_url` to the URL they give you.

### 3. Pair your phone

Press `Ctrl+Space Q` (dashboard or session; it starts the server if it's off)
to open the pairing dialog and scan the QR code with the
phone's camera. The page opens with the one-time code filled in; tap **Pair**.
Codes last five minutes and work once.

In Chrome, use **⋮ → Add to Home screen / Install app** to get an app icon.
Then open it and tap **Turn on notifications**; **Send test** checks the
whole path.

## Settings

| Key (`config.json`) | Default | Meaning |
|---|---|---|
| `remote_bind` | `127.0.0.1:47800` | Where the server listens. Keep a fixed port so the tunnel survives restarts. `0.0.0.0:47800` exposes it on your LAN as plain HTTP (works, but can't install or get notifications). |
| `remote_public_url` | unset | The HTTPS address the phone uses; encoded in the pairing QR. |

## Security

- Every device gets its own secret token at pairing; only a hash is stored
  (in `amf.db`). The pairing dialog's `v` lists paired devices; `d`, `d`
  revokes one, which cuts it off immediately and stops its notifications.
- Everything except the app shell itself and pairing requires the token.
  Terminal sockets authenticate in their first message, so tokens never
  appear in URLs or proxy logs.
- Notifications are end-to-end encrypted to the phone. They travel through
  the browser vendor's push service (Google's for Chrome), which can't read
  them. AMF signs them with its own key (VAPID) — no Firebase project or AMF
  relay is involved.
- Notifications keep arriving while AMF is running even with the server
  toggled off: the toggle controls whether the phone can reach AMF, not
  whether AMF can reach the phone. Revoke a device to stop them.

## What the phone won't do

Anything that would stop to ask a question at the desk is refused with a
reason rather than left waiting on a dialog nobody can see:

- starting or stopping a feature whose `on_start`/`on_stop` hook has a prompt
- deleting a feature whose worktree TODO list still has unfinished items (the
  desk asks where they should go)
- removing or deleting something that is open on the desk right now
- editing TODOs while the desk has the TODOs overlay open

The resource gate (agent limit / low memory) warns at the desk instead of
blocking, the same policy AMF uses for starts inside longer flows.
