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

### 2. Give it an HTTPS address with Tailscale

Installing the app and receiving notifications need HTTPS. The simplest way
to get it is [Tailscale](https://tailscale.com/download), which gives this
computer a private `https://<machine>.<tailnet>.ts.net` address that only
your own devices can open.

1. Install Tailscale on this computer and on your phone, and sign in to the
   same account on both (`tailscale up` on the computer).
2. In the Tailscale admin console under
   [DNS](https://login.tailscale.com/admin/dns), turn on **MagicDNS** and
   **HTTPS Certificates**.
3. Share AMF on your tailnet: press `Ctrl+Space Q`, then `t` in the pairing
   dialog, or run it yourself:

   ```sh
   tailscale serve --bg 47800
   ```

   The first time, Tailscale may print a link to approve Serve for your
   tailnet. The dialog shows it (`o` opens it); approve it, then press `t`
   again. Never use `tailscale funnel` for this: it puts AMF on the public
   internet.

That's all the configuration AMF needs. When Tailscale serves AMF's port,
the pairing QR uses its address automatically. You don't have to copy it
anywhere.

**Stuck?** Press `s` in the pairing dialog for the same steps, each ticked
off from what AMF sees on this computer, or run `amf doctor`.

#### Limit access to your own devices (recommended)

A new tailnet lets every device reach every port on every other. To allow
only your own devices to reach AMF, and only over HTTPS:

1. In [Access controls](https://login.tailscale.com/admin/acls/file), add
   this policy (`c` in the setup view copies it). If your policy still has
   the default `{"src": ["*"], "dst": ["*"], "ip": ["*"]}` grant, remove it,
   or it keeps letting everything reach everything. Any other devices you
   use Tailscale for then need grants of their own.

   ```jsonc
   {
     "tagOwners": {
       "tag:amf": ["autogroup:admin"],
     },
     "grants": [
       { "src": ["autogroup:member"], "dst": ["tag:amf"], "ip": ["tcp:443"] },
     ],
   }
   ```

2. In [Machines](https://login.tailscale.com/admin/machines), open this
   computer's ⋯ menu → **Edit ACL tags** and add `tag:amf`. (Save the policy
   first: it's what defines the tag.) Tagged machines also stop expiring
   their key, so AMF's computer won't drop off your tailnet on its own.

To have Tailscale check the rule every time the policy is saved, add a
`tests` block naming your login:

```jsonc
"tests": [
  { "src": "you@example.com", "accept": ["tag:amf:443"], "deny": ["tag:amf:22"] },
],
```

#### Tailscale in a non-standard place

AMF runs `tailscale` from your `PATH` (on macOS it also tries the app's
bundled CLI), talking to the daemon at its usual socket. If yours differs,
tell AMF in `~/.config/amf/config.json`:

```json
{
  "remote_tailscale_cli": "~/bin/tailscale",
  "remote_tailscale_socket": "~/.local/share/tailscale/tailscaled.sock"
}
```

The socket setting is for a `tailscaled` you start yourself with its own
`--socket`, for example where no service manager runs it for you:

```sh
tailscaled --tun=userspace-networking \
  --statedir="$HOME/.local/share/tailscale" \
  --socket="$HOME/.local/share/tailscale/tailscaled.sock" &
```

A daemon started this way stops when the machine (or WSL) shuts down, and
your phone loses access with it, so start it again before using AMF Remote.

#### Other tunnels

cloudflared, ngrok and the like work too: point them at `127.0.0.1:47800`
and set `remote_public_url` to the HTTPS address they give you. A
configured `remote_public_url` always wins over Tailscale's address.

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
| `remote_public_url` | unset | The HTTPS address the phone uses; encoded in the pairing QR. Unset, AMF uses the address Tailscale serves it on, if any. |
| `remote_tailscale_cli` | `tailscale` | The Tailscale CLI AMF asks for that address (falls back to the macOS app's bundled CLI). |
| `remote_tailscale_socket` | unset | `tailscaled`'s socket, when it isn't at the default — e.g. a userspace daemon under WSL. |

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
