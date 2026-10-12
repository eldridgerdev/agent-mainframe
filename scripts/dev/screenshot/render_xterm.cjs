#!/usr/bin/env node
// Render `tmux capture-pane -e -p` dumps to PNGs with xterm.js in headless
// Chromium: the terminal emulator the desktop app itself uses, so a frame
// shows what a terminal would draw instead of a reconstruction of it.
// render_ansi.py is the fallback for a machine without Node or a browser;
// render-captures.sh picks between the two.
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");

const USAGE = `Usage: render_xterm.cjs [--cols N] [--rows N] [--scale N] [--out-dir DIR] [--no-install] <capture.ansi>...
       render_xterm.cjs --install

Writes <capture>.png beside each input, or into --out-dir. --cols/--rows
default to the capture harness's 120x40; --scale is the pixel density
(default 2). --install fetches the pinned packages and a headless Chromium.
A render fetches missing packages itself unless --no-install is given, which
makes it fail instead.`;

// Keep playwright-core in step with capture-remote-dashboard.cjs so both
// share one downloaded browser. addon-webgl 0.19 is the release for xterm 6.0.
const PACKAGES = { "playwright-core": "1.63.0", "@xterm/xterm": "6.0.0", "@xterm/addon-webgl": "0.19.0" };
// Not XDG_STATE_HOME: amf-capture.sh points that at its scratch instance, and
// these would then be reinstalled, and deleted, on every run.
const toolsDir = path.join(
  process.env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache"), "amf", "screenshot-xterm");
const modules = path.join(toolsDir, "node_modules");
const BASE_FONT_SIZE = 14;
// The one font CI and a workstation both have.
const FONT = "DejaVu Sans Mono";

function missingPackages() {
  return Object.entries(PACKAGES).filter(([name, version]) => {
    try {
      return JSON.parse(fs.readFileSync(path.join(modules, name, "package.json"), "utf8")).version !== version;
    } catch {
      return true;
    }
  });
}

function installPackages() {
  const missing = missingPackages();
  if (missing.length) {
    // npm's progress goes to stderr: stdout carries only the "wrote" lines.
    execFileSync("npm", ["install", "--prefix", toolsDir, "--no-audit", "--no-fund",
      "--package-lock=false", ...missing.map(([name, version]) => `${name}@${version}`)],
    { stdio: ["ignore", 2, 2] });
  }
}

// The desktop app's dark terminal palette, read from its source rather than
// copied, so a screenshot of the TUI and the app's own terminal pane agree.
function guiTerminalTheme() {
  const file = path.join(__dirname, "../../../gui/src/terminalTheme.ts");
  const block = fs.readFileSync(file, "utf8").match(/DARK_TERMINAL_THEME[^{]*\{([^}]*)\}/);
  assert(block, `DARK_TERMINAL_THEME was not found in ${file}`);
  const theme = Object.fromEntries(
    [...block[1].matchAll(/(\w+):\s*"(#[0-9a-fA-F]+)"/g)].map(([, key, value]) => [key, value]));
  for (const key of ["background", "foreground", "black", "brightWhite"]) {
    assert(theme[key], `DARK_TERMINAL_THEME in ${file} has no readable "${key}"`);
  }
  return theme;
}

async function render(inputs, { cols, rows, scale, outDir, install }) {
  if (install) {
    installPackages();
  } else {
    assert(!missingPackages().length,
      `the xterm.js packages are not installed in ${toolsDir}; run render_xterm.cjs --install`);
  }
  const { chromium } = require(path.join(modules, "playwright-core"));
  const theme = guiTerminalTheme();
  const browser = await chromium.launch({
    ...(process.env.AMF_PWA_BROWSER ? { executablePath: process.env.AMF_PWA_BROWSER } : {}),
    headless: true,
    // Software WebGL for a runner with no GPU. A workstation renders the
    // same without them.
    args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"],
  });
  try {
    // Density comes from the font size, not deviceScaleFactor: at a device
    // scale of 2 the WebGL canvas draws a zoomed quarter of the screen here.
    const page = await browser.newPage({ deviceScaleFactor: 1 });
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.setContent(
      `<!doctype html><body style="margin:0;background:${theme.background}"><div id="terminal"></div>`);
    await page.addStyleTag({ path: path.join(modules, "@xterm/xterm/css/xterm.css") });
    await page.addScriptTag({ path: path.join(modules, "@xterm/xterm/lib/xterm.js") });
    await page.addScriptTag({ path: path.join(modules, "@xterm/addon-webgl/lib/addon-webgl.js") });

    const { fontFound, ...size } = await page.evaluate(({ cols, rows, fontSize, font, theme }) => {
      const term = new Terminal({
        cols, rows, fontSize, theme,
        fontFamily: `"${font}", monospace`,
        // capture-pane ends each row with a bare line feed.
        convertEol: true,
        cursorBlink: false,
        scrollback: 0,
        // Same value as terminalAppearance() in gui/src/themes.ts for dark mode.
        minimumContrastRatio: 4.5,
      });
      term.open(document.getElementById("terminal"));
      // Not optional. The WebGL renderer draws box and block characters as
      // geometry; the default DOM renderer takes them from the font and leaves
      // seams between cells. This throws where WebGL is unavailable, which is
      // the caller's cue to use render_ansi.py instead.
      term.loadAddon(new WebglAddon.WebglAddon());
      // A context can also be lost later, and a frame drawn after that is
      // blank or stale. The addon's own onContextLoss is no use here: it
      // waits three seconds for the context to come back before firing.
      document.addEventListener("webglcontextlost", () => { window.webglContextLost = true; }, true);
      window.term = term;
      // A font that is not installed falls through to the generic family
      // without any error. Two different generics measure the same only if
      // neither was used.
      const width = generic => {
        const context = document.createElement("canvas").getContext("2d");
        context.font = `${fontSize}px "${font}", ${generic}`;
        return context.measureText("mmmmmmmmmmlli").width;
      };
      const box = document.querySelector(".xterm-screen").getBoundingClientRect();
      return {
        width: Math.ceil(box.width), height: Math.ceil(box.height),
        fontFound: width("monospace") === width("serif"),
      };
    }, { cols, rows, fontSize: BASE_FONT_SIZE * scale, font: FONT, theme });
    if (!fontFound) {
      // Not an error: the Pillow fallback needs the same font, and without it
      // draws a bitmap one, which is worse than any monospace font here.
      console.error(`render_xterm: warning: ${FONT} is not installed, so these frames use the browser's `
        + "default monospace font; their cell size and pixel dimensions will not match CI's.");
    }
    await page.setViewportSize(size);

    for (const input of inputs) {
      // The line feed after the last row would scroll the first one away.
      const ansi = fs.readFileSync(input, "utf8").replace(/\n$/, "");
      await page.evaluate(async ansi => {
        window.term.reset();
        // Hide the cursor: capture-pane records no cursor position, so the
        // emulator's own would sit wherever the dump happened to end.
        await new Promise(done => window.term.write("\x1b[?25l" + ansi, done));
        await new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)));
      }, ansi);
      assert.deepEqual(errors, [], `xterm.js reported an error while rendering ${input}`);
      const output = path.join(outDir || path.dirname(input), `${path.basename(input, ".ansi")}.png`);
      const png = await page.locator(".xterm-screen").screenshot();
      // Checked after the screenshot, and before anything is written, so a
      // context lost while it was being taken counts too.
      assert(!(await page.evaluate(() => window.webglContextLost)),
        `the WebGL context was lost while rendering ${input}`);
      fs.mkdirSync(path.dirname(output), { recursive: true });
      fs.writeFileSync(output, png);
      console.log(`wrote ${output} (${cols}x${rows} cells)`);
    }
  } finally {
    await browser.close();
  }
}

function parseArgs(argv) {
  const options = { cols: 120, rows: 40, scale: 2, outDir: null, install: true };
  const inputs = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--out-dir") {
      options.outDir = argv[++i];
      assert(options.outDir, "--out-dir requires a directory");
    } else if (arg === "--no-install") {
      options.install = false;
    } else if (["--cols", "--rows", "--scale"].includes(arg)) {
      const value = Number(argv[++i]);
      assert(Number.isInteger(value) && value > 0, `${arg} requires a positive integer`);
      options[arg.slice(2)] = value;
    } else if (arg.startsWith("--")) {
      throw new Error(`unknown option: ${arg}`);
    } else {
      inputs.push(arg);
    }
  }
  assert(inputs.length, "no .ansi captures given");
  return { inputs, options };
}

(async () => {
  const argv = process.argv.slice(2);
  if (argv.includes("-h") || argv.includes("--help")) {
    console.log(USAGE);
    return;
  }
  if (argv[0] === "--install") {
    installPackages();
    // A local run can reuse an installed browser. CI also needs the system
    // libraries a headless browser links against.
    if (!process.env.AMF_PWA_BROWSER) {
      execFileSync(process.execPath, [path.join(modules, "playwright-core", "cli.js"), "install",
        "--only-shell", ...(process.env.CI ? ["--with-deps"] : []), "chromium"], { stdio: ["ignore", 2, 2] });
    }
    return;
  }
  const { inputs, options } = parseArgs(argv);
  await render(inputs, options);
})().catch(error => {
  console.error(`render_xterm: ${error.message}`);
  process.exitCode = 1;
});
