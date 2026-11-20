#!/usr/bin/env node
// Capture marketing screenshots of AMF Remote (the phone PWA) for the README
// and agentmainframe.dev.
//
// The page is the real shell from src/remote_web/, loaded in headless
// Chromium at phone size. Every request it makes (status, actions, diff,
// push, and the terminal WebSocket) is answered here from sample data, so the
// capture needs no AMF process, tmux, paired device or agent run, and it
// never reads the user's database. For proof that the real server serves the
// shell, see capture-remote-dashboard.cjs and its scenario.
//
//   node scripts/dev/screenshot/capture-remote-showcase.cjs <output-directory>
//
// Writes one PNG per view (dark and a light home), plus showcase.png: the
// key views side by side, for the README and the site.
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");

// Keep in step with capture-remote-dashboard.cjs and render_xterm.cjs.
const PLAYWRIGHT_VERSION = "1.63.0";
const toolsDir = path.join(process.env.XDG_STATE_HOME || os.tmpdir(), "amf", "screenshot-playwright");
const playwrightPath = path.join(toolsDir, "node_modules", "playwright-core");

function loadPlaywright() {
  if (!fs.existsSync(path.join(playwrightPath, "package.json"))) {
    execFileSync("npm", ["install", "--prefix", toolsDir, "--no-audit", "--no-fund",
      "--package-lock=false", `playwright-core@${PLAYWRIGHT_VERSION}`], { stdio: "inherit" });
  }
  return require(playwrightPath);
}

const outDir = process.argv[2];
assert(outDir, "Usage: capture-remote-showcase.cjs <output-directory>");
fs.mkdirSync(outDir, { recursive: true });

const WEB = path.resolve(__dirname, "../../../src/remote_web");
// A tailnet-style HTTPS origin: a secure context, like a real install. No
// request leaves the browser; every URL is fulfilled below.
const ORIGIN = "https://desk.example.ts.net";
const CONTENT_TYPES = {
  ".html": "text/html", ".js": "text/javascript", ".css": "text/css",
  ".png": "image/png", ".webmanifest": "application/manifest+json",
};

// ---- Sample data ------------------------------------------------------------

const session = (id, label, kind, live = true) => ({ id, label, kind, live });
const feature = (project, id, name, status, agent, extra = {}) => ({
  project_name: project, feature_id: id, feature_name: name, branch: `feature/${id}`,
  status, agent, needs_attention: false, attention_reason: null, attention_detail: null,
  sessions: [], summary: null, nickname: null, ...extra,
});

const snapshot = {
  generated_at: new Date().toISOString(),
  projects: [
    { name: "agent-mainframe", preferred_agent: "claude" },
    { name: "checkout-web", preferred_agent: "codex" },
    { name: "billing-api", preferred_agent: "claude" },
  ],
  features: [
    feature("agent-mainframe", "offline-shell", "offline-shell", "active", "claude", {
      needs_attention: true, attention_reason: "Question",
      attention_detail: "Run the service worker tests?",
      summary: "Serve the cached app shell when the desk is unreachable, and say so instead of hanging on the splash screen.",
      sessions: [session("s-claude", "Claude", "claude"), session("s-shell", "Terminal", "terminal"),
        session("s-nvim", "Neovim", "nvim", false)],
    }),
    feature("checkout-web", "apple-pay", "apple-pay", "idle", "codex", {
      needs_attention: true, attention_reason: "Completed", attention_detail: "Ready for your review",
      sessions: [session("s-codex", "Codex", "codex")],
    }),
    feature("agent-mainframe", "push-actions", "push-actions", "active", "claude", {
      sessions: [session("s-claude-2", "Claude", "claude")],
    }),
    feature("checkout-web", "cart-a11y", "cart-a11y", "active", "opencode", {
      sessions: [session("s-oc", "OpenCode", "opencode")],
    }),
    feature("checkout-web", "lighthouse-ci", "lighthouse-ci", "stopped", "codex"),
    feature("billing-api", "idempotency-keys", "idempotency-keys", "active", "pi", {
      sessions: [session("s-pi", "Pi", "pi")],
    }),
    feature("billing-api", "invoice-pdf", "invoice-pdf", "stopped", "claude"),
  ],
};

const diff = {
  branch: "feature/offline-shell",
  base_ref: "main",
  total_additions: 64,
  total_deletions: 9,
  files: [
    { path: "src/remote_web/sw.js", status: "modified", additions: 31, deletions: 6, is_binary: false,
      patch: [
        "@@ -1,12 +1,18 @@",
        " // Caches the app shell so an installed AMF Remote opens even when AMF is",
        "-const CACHE = \"amf-remote-v6\";",
        "+const CACHE = \"amf-remote-v7\";",
        "+// How long a page load waits on AMF before the cached shell is used.",
        "+const NETWORK_TIMEOUT_MS = 4000;",
        " const SHELL = [\"/\", \"/app.js\", \"/app.css\", \"/manifest.webmanifest\"];",
        " ",
        "@@ -40,6 +46,14 @@ async function networkFirst(request, key) {",
        "-  return fetch(request);",
        "+  const answer = await Promise.race([",
        "+    network,",
        "+    new Promise((resolve) => setTimeout(resolve, NETWORK_TIMEOUT_MS, null)),",
        "+  ]);",
        "+  if (answer?.ok) return answer;",
        "+  return (await caches.match(key)) ?? network;",
        " }",
      ].join("\n") },
    { path: "src/remote_web/offline.js", status: "added", additions: 27, deletions: 0, is_binary: false,
      patch: "@@ -0,0 +1,27 @@\n+export function showOffline(detail) {" },
    { path: "src/remote_web/app.css", status: "modified", additions: 6, deletions: 1, is_binary: false,
      patch: "@@ -10,3 +10,8 @@\n+.offline { color: var(--danger); }" },
    { path: "src/remote_web/legacy-splash.html", status: "deleted", additions: 0, deletions: 2, is_binary: false,
      patch: "@@ -1,2 +0,0 @@\n-<p>Loading…</p>" },
  ],
};

const todos = {
  editable: true,
  lists: [
    { label: "This worktree", scope: "worktree",
      scratchpad: "Installed app must open in < 5 s with the desk asleep.",
      items: [
        { id: 1, title: "Race the network against a 4 s timeout", body: null, status: "completed" },
        { id: 2, title: "Show \"Can't reach AMF\" from the cached shell", body: "Instead of the splash screen", status: "in_progress" },
        { id: 3, title: "Never cache a tunnel's 502 page", body: null, status: "not_started" },
      ] },
    { label: "Project", scope: "project", scratchpad: null,
      items: [
        { id: 4, title: "Lighthouse PWA audit in CI", body: null, status: "not_started" },
      ] },
  ],
};

// One terminal frame: rows joined by \n with SGR colors, like
// remote_terminal.rs sends. An agent paused on a permission question.
const E = (codes) => `\x1b[${codes}m`;
const R = E(0), DIM = E(2), B = E(1), TEAL = E("38;5;80"), AMBER = E("38;5;215"), GRN = E(32), RED = E(31);
const frameRows = [
  `${TEAL}●${R} I'll make the shell fall back to the cache`,
  `  when the desk doesn't answer in time.`,
  ``,
  `${TEAL}●${R} ${B}Update${R}(src/remote_web/sw.js)`,
  `  ${DIM}└${R}  Updated with ${GRN}31 additions${R}, ${RED}6 removals${R}`,
  `     ${GRN}+ const NETWORK_TIMEOUT_MS = 4000;${R}`,
  `     ${GRN}+ const answer = await Promise.race([${R}`,
  `     ${GRN}+   network, timeout(NETWORK_TIMEOUT_MS),${R}`,
  `     ${GRN}+ ]);${R}`,
  `     ${RED}- return fetch(request);${R}`,
  ``,
  `${TEAL}●${R} Next I'll run the remote server tests to`,
  `  check the shell is still served without a`,
  `  token.`,
  ``,
  `${AMBER}╭────────────────────────────────────────╮${R}`,
  `${AMBER}│${R} ${B}Run this command?${R}                      ${AMBER}│${R}`,
  `${AMBER}│${R}                                        ${AMBER}│${R}`,
  `${AMBER}│${R}   cargo test remote_server             ${AMBER}│${R}`,
  `${AMBER}│${R}                                        ${AMBER}│${R}`,
  `${AMBER}│${R} ${TEAL}❯ 1. Yes${R}                               ${AMBER}│${R}`,
  `${AMBER}│${R}   2. Yes, and don't ask again          ${AMBER}│${R}`,
  `${AMBER}│${R}   3. No, and tell the agent what to do ${AMBER}│${R}`,
  `${AMBER}╰────────────────────────────────────────╯${R}`,
];
const frame = {
  type: "frame", cols: 42, rows: frameRows.length, ansi: frameRows.join("\n"),
  cursor_x: 0, cursor_y: 0, cursor_visible: false,
};

// ---- Browser ------------------------------------------------------------------

async function mockAmf(page) {
  await page.route(`${ORIGIN}/**`, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const p = url.pathname;
    if (p === "/status") return route.fulfill({ json: snapshot });
    if (p === "/push/key") return route.fulfill({ json: { public_key: "BAAA" } });
    if (p.startsWith("/push/")) return route.fulfill({ json: {} });
    if (/^\/features\/[^/]+\/diff$/.test(p)) return route.fulfill({ json: diff });
    if (p === "/actions") {
      const body = request.postDataJSON();
      const message = body.action === "list_todos" ? todos : null;
      return route.fulfill({ json: { message } });
    }
    const file = p === "/" ? "index.html" : p.slice(1);
    const full = path.join(WEB, file);
    if (!full.startsWith(WEB + path.sep) || !fs.existsSync(full)) return route.fulfill({ status: 404 });
    return route.fulfill({
      body: fs.readFileSync(full),
      contentType: CONTENT_TYPES[path.extname(full)] ?? "application/octet-stream",
    });
  });
  await page.routeWebSocket(/\/sessions\/[^/]+\/terminal$/, (ws) => {
    ws.onMessage((message) => {
      if (JSON.parse(message).type === "auth") ws.send(JSON.stringify(frame));
    });
  });
}

// A paired device with notifications already on: a stored credential, and a
// push subscription the page can find without a real service worker.
function pairedDeviceScript() {
  localStorage.setItem("amf-remote-credential", JSON.stringify({ deviceId: "demo", token: "demo" }));
  localStorage.setItem("amf-remote-term-wrap", "off");
  localStorage.setItem("amf-remote-term-font", "11");
  Object.defineProperty(Notification, "permission", { get: () => "granted" });
  const subscription = {
    options: { applicationServerKey: null },
    toJSON: () => ({ endpoint: "https://push.example/demo", keys: { p256dh: "x", auth: "y" } }),
    unsubscribe: async () => true,
  };
  const registration = { pushManager: { getSubscription: async () => subscription } };
  if (navigator.serviceWorker) {
    Object.defineProperty(navigator.serviceWorker, "ready", { get: () => Promise.resolve(registration) });
    navigator.serviceWorker.register = async () => registration;
  }
}

async function newPhone(browser, { colorScheme = "dark", paired = true } = {}) {
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 }, deviceScaleFactor: 2,
    isMobile: true, hasTouch: true, colorScheme, serviceWorkers: "block",
    locale: "en-US", timezoneId: "UTC",
  });
  if (paired) await context.addInitScript(pairedDeviceScript);
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await mockAmf(page);
  return { context, page, errors };
}

async function settle(page) {
  await page.evaluate(() => document.fonts.ready);
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
}

(async () => {
  const { chromium } = loadPlaywright();
  const browser = await chromium.launch({
    ...(process.env.AMF_PWA_BROWSER ? { executablePath: process.env.AMF_PWA_BROWSER } : {}),
    headless: true,
  });
  const shots = {};
  async function shoot(page, errors, name) {
    await settle(page);
    assert.deepEqual(errors, [], `page errors before ${name}`);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true,
      `${name} scrolls sideways`);
    const file = path.join(outDir, `${name}.png`);
    await page.screenshot({ path: file, animations: "disabled" });
    shots[name] = file;
    console.log(`captured ${name}.png`);
  }

  try {
    {
      const { context, page, errors } = await newPhone(browser, { paired: false });
      await page.goto(`${ORIGIN}/`);
      await page.getByLabel("Device name").fill("Pixel 9");
      await page.getByLabel("Pairing code").fill("482915");
      await shoot(page, errors, "01-pairing");
      await context.close();
    }

    const { context, page, errors } = await newPhone(browser);
    await page.goto(`${ORIGIN}/`);
    await page.getByText("Notifications on").waitFor();
    await page.locator("#attention li").nth(1).waitFor();
    await shoot(page, errors, "02-home");

    await page.goto(`${ORIGIN}/#/f/offline-shell`);
    await page.locator("#sessions li").first().waitFor();
    await shoot(page, errors, "03-feature");

    await page.goto(`${ORIGIN}/#/s/s-claude`);
    await page.getByText("Run this command?").waitFor();
    await page.locator("#send-text").fill("Yes — and add a test for the 502 case");
    await shoot(page, errors, "04-session");

    await page.goto(`${ORIGIN}/#/d/offline-shell`);
    await page.locator(".diff-file").first().waitFor();
    await page.locator(".diff-file summary").first().click();
    await shoot(page, errors, "05-changes");

    await page.goto(`${ORIGIN}/#/t/offline-shell`);
    await page.locator(".todo-item").first().waitFor();
    await shoot(page, errors, "06-todos");
    await context.close();

    {
      const light = await newPhone(browser, { colorScheme: "light" });
      await light.page.goto(`${ORIGIN}/`);
      await light.page.locator("#attention li").nth(1).waitFor();
      await shoot(light.page, light.errors, "07-home-light");
      await light.context.close();
    }

    // The strip: four phones, each with a caption, on the site's background.
    const strip = [
      ["02-home", "See who needs you"],
      ["04-session", "Answer the agent"],
      ["05-changes", "Review the diff"],
      ["06-todos", "Hand out the next task"],
    ];
    const phones = strip.map(([name, caption]) => {
      const data = fs.readFileSync(shots[name]).toString("base64");
      return `<figure><img src="data:image/png;base64,${data}"><figcaption>${caption}</figcaption></figure>`;
    }).join("");
    const composer = await browser.newContext({ viewport: { width: 1680, height: 900 }, deviceScaleFactor: 1 });
    const canvas = await composer.newPage();
    await canvas.setContent(`<!doctype html><html><head><style>
      body { margin: 0; background: #0b0e11; font: 600 22px system-ui, sans-serif; color: #d8dee6; }
      main { display: flex; gap: 40px; justify-content: center; padding: 48px 40px 40px; }
      figure { margin: 0; text-align: center; }
      img { width: 360px; display: block; border-radius: 36px; border: 10px solid #1d232b;
            box-shadow: 0 0 0 1px #2b333d, 0 30px 60px rgba(0,0,0,.5); }
      figcaption { margin-top: 22px; }
    </style></head><body><main>${phones}</main></body></html>`);
    await canvas.evaluate(() => Promise.all([...document.images].map((i) => i.decode())));
    await canvas.locator("main").screenshot({ path: path.join(outDir, "showcase.png") });
    console.log("captured showcase.png");
    await composer.close();
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
