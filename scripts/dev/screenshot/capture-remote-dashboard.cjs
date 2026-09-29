#!/usr/bin/env node
// Capture the real PWA shell from the screenshot harness's isolated AMF
// server, using deterministic sample status data and no live agent runs.
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");

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

if (process.argv[2] === "--install-browser") {
  loadPlaywright();
  // Local captures can reuse an installed browser. CI installs a headless
  // browser and its system libraries before the short-lived pairing code.
  if (!process.env.AMF_PWA_BROWSER) {
    execFileSync(process.execPath, [path.join(playwrightPath, "cli.js"), "install",
      "--only-shell", ...(process.env.CI ? ["--with-deps"] : []), "chromium"], { stdio: "inherit" });
  }
  process.exit(0);
}

const [session, outDir] = process.argv.slice(2);
assert(session && outDir, "Usage: capture-remote-dashboard.cjs <tmux-session> <output-directory>");
const pane = execFileSync("tmux", ["capture-pane", "-p", "-t", session], { encoding: "utf8" });
const url = pane.match(/http:\/\/127\.0\.0\.1:\d+/)?.[0];
const code = pane.match(/\b\d(?: \d){5}\b/)?.[0].replaceAll(" ", "");
assert(url, "The isolated AMF pairing URL must be visible.");
assert(code, "The isolated AMF pairing code must be visible.");

const makeFeature = (project, name, status, agent, needsAttention = false) => ({
  project_name: project,
  feature_id: name.toLowerCase().replaceAll(" ", "-"),
  feature_name: name,
  status, agent, needs_attention: needsAttention,
  attention_reason: needsAttention ? "Completed" : null,
  attention_detail: needsAttention ? "Ready for your review" : null,
  sessions: [],
});
const snapshot = {
  projects: [{ name: "agent-mainframe" }, { name: "demo-api" }],
  features: [
    makeFeature("agent-mainframe", "Remote dashboard", "active", "codex", true),
    makeFeature("agent-mainframe", "Terminal polish", "active", "claude"),
    makeFeature("demo-api", "Authentication", "idle", "codex"),
    makeFeature("demo-api", "Release cleanup", "stopped", "pi"),
  ],
};

(async () => {
  const { chromium } = loadPlaywright();
  const browser = await chromium.launch({
    ...(process.env.AMF_PWA_BROWSER ? { executablePath: process.env.AMF_PWA_BROWSER } : {}),
    headless: true,
  });
  try {
    const context = await browser.newContext({
      viewport: { width: 390, height: 844 }, deviceScaleFactor: 2,
      isMobile: true, hasTouch: true, colorScheme: "dark", serviceWorkers: "block",
    });
    const errors = [];
    const page = await context.newPage();
    page.on("pageerror", error => errors.push(error.message));
    // Pairing and assets use the real server; only the feature feed is a
    // fixture. Never borrow a real device token or inspect the user's DB.
    await page.route(`${url}/status`, route => route.fulfill({ json: snapshot }));
    await page.goto(`${url}/?code=${code}`);
    await page.getByLabel("Device name").fill("Screenshot browser");
    await page.getByRole("button", { name: "Pair", exact: true }).click();
    const amf = () => page.getByRole("button", { name: "agent-mainframe 2 features", exact: true });
    const api = () => page.getByRole("button", { name: "demo-api 2 features", exact: true });
    await amf().waitFor();
    await page.evaluate(() => document.fonts.ready);
    const notes = [];
    async function capture(name, caption) {
      assert.equal(await page.locator("#attention li").count(), 1);
      assert.equal(await page.locator("#attention li").isVisible(), true);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      assert.deepEqual(errors, []);
      const file = path.join(outDir, name);
      await page.screenshot({ path: file, fullPage: true });
      fs.writeFileSync(file.replace(".png", ".txt"), await page.locator("#view-home").innerText());
      notes.push({ file: name.replace(".png", ".ansi"), note: caption });
      console.log(`PASS: ${caption}`);
    }

    assert.equal(await amf().getAttribute("aria-expanded"), "true");
    assert.equal(await api().getAttribute("aria-expanded"), "true");
    assert.equal(await page.getByText("Terminal polish", { exact: true }).isVisible(), true);
    assert.equal(await page.getByText("Authentication", { exact: true }).isVisible(), true);
    await capture("001-expanded-projects.png",
      "Both sample projects start expanded, with feature counts and the needs-attention section above them.");

    await amf().tap();
    await page.evaluate(() => refresh());
    assert.equal(await amf().getAttribute("aria-expanded"), "false");
    assert.equal(await api().getAttribute("aria-expanded"), "true");
    assert.equal(await page.getByText("Terminal polish", { exact: true }).isVisible(), false);
    assert.equal(await page.getByText("Authentication", { exact: true }).isVisible(), true);
    assert.equal(await amf().evaluate(el =>
      getComputedStyle(document.getElementById(el.getAttribute("aria-controls"))).display), "none");
    await capture("002-one-project-collapsed.png",
      "Tapping agent-mainframe hides its feature list through a refresh; demo-api and the attention item stay visible.");

    await api().tap();
    await page.reload();
    await amf().waitFor();
    assert.equal(await amf().getAttribute("aria-expanded"), "false");
    assert.equal(await api().getAttribute("aria-expanded"), "false");
    assert.equal(await page.getByText("Terminal polish", { exact: true }).isVisible(), false);
    assert.equal(await page.getByText("Authentication", { exact: true }).isVisible(), false);
    assert.deepEqual(await page.evaluate(() =>
      JSON.parse(localStorage.getItem("amf-remote-collapsed-projects"))), ["agent-mainframe", "demo-api"]);
    await capture("003-collapsed-after-reload.png",
      "Both sample projects stay collapsed after a page reload, with counts and the needs-attention item still visible.");

    // Keep the asserted pairing setup as internal diagnostics. The gallery
    // should contain only the three browser frames that prove this change.
    const setupDir = path.join(outDir, "setup");
    fs.mkdirSync(setupDir, { recursive: true });
    for (const extension of ["ansi", "txt"]) {
      const setup = `001-remote-pairing-ready.${extension}`;
      fs.renameSync(path.join(outDir, setup), path.join(setupDir, setup));
    }
    fs.writeFileSync(path.join(outDir, "capture-notes.jsonl"),
      notes.map(note => JSON.stringify(note)).join("\n") + "\n");
    await context.close();
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
