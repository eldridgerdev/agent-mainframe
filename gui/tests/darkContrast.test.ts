import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { DARK_TERMINAL_THEME } from "../src/terminalTheme";
import { blocks, token, parse, over, contrast, type Rgb } from "./contrast";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const dark = blocks(css).dark;
const surfaces = ["bg", "bg-sidebar", "surface", "surface-2", "surface-3"];
const backgrounds = Object.fromEntries(surfaces.map((name) => [name, parse(token(dark, name)).rgb]));
const failures: string[] = [];
function check(label: string, fg: Rgb, bg: Rgb, minimum = 4.5) {
  const ratio = contrast(fg, bg);
  if (ratio < minimum) failures.push(`${label}: ${ratio.toFixed(2)} < ${minimum}`);
}

it("keeps dark body text, status/context colours and chrome readable on real surfaces", () => {
  failures.length = 0;
  for (const [name, bg] of Object.entries(backgrounds)) {
    for (const text of ["text", "text-muted", "text-faint", "accent", "green", "amber", "red"]) {
      check(`${text} on ${name}`, parse(token(dark, text)).rgb, bg);
    }
    for (const border of ["border", "border-strong"]) {
      check(`${border} on ${name}`, parse(token(dark, border)).rgb, bg, 3);
    }
    for (const colour of ["accent", "green", "amber", "red"]) {
      const tinted = over(token(dark, `${colour}-soft`), bg);
      check(`${colour} badge on ${name}`, parse(token(dark, colour)).rgb, tinted);
      for (const text of ["text", "text-muted", "text-faint"]) {
        check(`${text} on ${colour}-soft/${name}`, parse(token(dark, text)).rgb, tinted);
      }
    }
  }
  for (const colour of ["accent", "accent-hover", "green", "amber", "red", "border-strong"]) {
    check(`filled ${colour} control`, parse(token(dark, "accent-fg")).rgb, parse(token(dark, colour)).rgb);
  }
  expect(dark).toContain(".switch::after { background: var(--accent-fg); }");
  expect(failures).toEqual([]);
});

it("checks merged PR, harness icons and every supervibe gradient stop", () => {
  failures.length = 0;
  // These dark-only rules follow the base selectors in the shipped stylesheet.
  const overrides = css.slice(css.lastIndexOf("@media (prefers-color-scheme: dark)"));
  const merged = overrides.match(/\.tree-pr\.pr-merged \{ color: (#[\da-f]+); background: ([^;]+);/)!;
  const icons = ["kind-claude", "kind-vscode"].map((kind) =>
    overrides.match(new RegExp(`\\.${kind} \\{ color: (#[\\da-f]+);`))![1]);
  const gradient = overrides.match(/background: linear-gradient\(90deg, ([^)]+)\)/)![1].split(", ");
  for (const [name, bg] of Object.entries(backgrounds)) {
    check(`merged PR on ${name}`, parse(merged[1]).rgb, over(merged[2], bg));
    for (const colour of icons) check(`icon ${colour} on ${name}`, parse(colour).rgb, bg);
  }
  for (const stop of gradient) check(`supervibe on ${stop}`, parse("#ffffff").rgb, parse(stop).rgb);
  expect(failures).toEqual([]);
});

it("keeps terminal ANSI, selection, cursor and dim text readable on the page background", () => {
  failures.length = 0;
  const theme = DARK_TERMINAL_THEME;
  expect(theme.background).toBe(token(dark, "bg"));
  expect(theme.background).toBe(token(dark, "terminal-bg"));
  const bg = parse(theme.background!).rgb;
  for (const [role, colour] of Object.entries(theme)) {
    if (["background", "cursorAccent", "selectionBackground", "selectionForeground"].includes(role)) continue;
    check(`terminal ${role}`, parse(colour!).rgb, bg, role === "cursor" ? 3 : 4.5);
  }
  check("selection", parse(theme.selectionForeground!).rgb, parse(theme.selectionBackground!).rgb);
  check("cursor glyph", parse(theme.cursorAccent!).rgb, parse(theme.cursor!).rgb);
  expect(failures).toEqual([]);
});

it("keeps independent agent-sidebar accents and VS Code glyphs readable", () => {
  failures.length = 0;
  const sidebar = blocks(readFileSync(new URL("../src/sessionSidebar.css", import.meta.url), "utf8")).dark;
  const sessions = readFileSync(new URL("../src/sessions.css", import.meta.url), "utf8");
  const vscode = blocks(sessions).dark.match(/color: (#[\da-f]+);/)![1];
  for (const role of ["harness-claude", "harness-codex", "harness-opencode", "accent-pr", "accent-summary", "accent-prompt"]) {
    for (const [name, bg] of Object.entries(backgrounds)) {
      check(`agent sidebar ${role} on ${name}`, parse(token(sidebar, `sb-${role}`)).rgb, bg);
    }
  }
  for (const [name, bg] of Object.entries(backgrounds)) check(`VS Code icon on ${name}`, parse(vscode).rgb, bg);
  expect(failures).toEqual([]);
});
