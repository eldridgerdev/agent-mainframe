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

it("keeps the new dark control, on-colour and disabled tokens readable in both dark blocks", () => {
  const themed = css.slice(css.lastIndexOf(':root[data-theme="dark"] {'));
  for (const block of [dark, themed]) {
    failures.length = 0;
    for (const [name, bg] of Object.entries(backgrounds)) check(`border-control on ${name}`, parse(token(block, "border-control")).rgb, bg, 3);
    for (const [fg, fill] of [["danger-fg", "red"], ["warning-fg", "amber"], ["success-fg", "green"]]) {
      check(`${fg} on ${fill}`, parse(token(block, fg)).rgb, parse(token(block, fill)).rgb);
    }
    const disabledFg = parse(token(block, "disabled-fg")).rgb;
    check("disabled-fg on disabled-bg", disabledFg, parse(token(block, "disabled-bg")).rgb, 3);
    for (const name of ["surface", "surface-2"]) check(`disabled-fg on ${name}`, disabledFg, backgrounds[name], 3);
    expect(failures).toEqual([]);
  }
});

it("keeps the kind, merged-PR and supervibe tokens readable in both dark blocks", () => {
  const themed = css.slice(css.lastIndexOf(':root[data-theme="dark"] {'));
  for (const block of [dark, themed]) {
    failures.length = 0;
    // Resolve a `var(--x)` alias to the token it points at.
    const resolve = (name: string) => {
      const value = token(block, name);
      return parse(value.startsWith("var(") ? token(block, value.slice(6, -1)) : value).rgb;
    };
    for (const [name, bg] of Object.entries(backgrounds)) {
      for (const kind of ["kind-nvim", "kind-vscode", "purple"]) check(`${kind} on ${name}`, resolve(kind), bg);
      check(`purple on purple-soft/${name}`, parse(token(block, "purple")).rgb, over(token(block, "purple-soft"), bg));
    }
    // Tree session rows sit on the active-row tint over the sidebar.
    const active = over(token(block, "accent-soft"), backgrounds["bg-sidebar"]);
    for (const kind of ["kind-nvim", "kind-vscode", "kind-claude", "kind-codex", "kind-opencode", "kind-pi", "purple"]) {
      // --sb-harness-* aliases live in sessionSidebar.css and are tested there.
      if (!token(block, kind).startsWith("var(--sb-")) check(`${kind} on the active row`, resolve(kind), active);
    }
    expect(failures).toEqual([]);
  }
  // Only the token form remains: no per-mode literal overrides.
  expect(css).toContain("background: var(--purple-soft); color: var(--purple);");
  expect(css).not.toMatch(/\[data-theme="(light|dark)"\]\)? \.(kind-|tree-pr)/);
});

it("draws the supervibe label on the surface with a decorative rainbow ring", () => {
  failures.length = 0;
  const rule = css.match(/\.tree-chip-mode\.mode-supervibe \{([^}]+)\}/)![1];
  expect(rule).toContain("linear-gradient(var(--surface), var(--surface)) padding-box");
  expect(rule).toMatch(/linear-gradient\(90deg, [^)]+\) border-box/);
  expect(rule).toContain("color: var(--text)");
  for (const [name, bg] of Object.entries(backgrounds)) {
    if (name === "surface") check("supervibe text on surface", parse(token(dark, "text")).rgb, bg);
  }
  expect(failures).toEqual([]);
});

it("keeps terminal foreground colors, selection and cursor readable on the page background", () => {
  failures.length = 0;
  const theme = DARK_TERMINAL_THEME;
  expect(theme.background).toBe(token(dark, "bg"));
  expect(theme.background).toBe(token(dark, "terminal-bg"));
  const bg = parse(theme.background!).rgb;
  for (const [role, colour] of Object.entries(theme)) {
    if (["black", "brightBlack", "background", "cursorAccent", "selectionBackground", "selectionForeground"].includes(role)) continue;
    check(`terminal ${role}`, parse(colour!).rgb, bg, role === "cursor" ? 3 : 4.5);
  }
  check("selection", parse(theme.selectionForeground!).rgb, parse(theme.selectionBackground!).rgb);
  check("cursor glyph", parse(theme.cursorAccent!).rgb, parse(theme.cursor!).rgb);
  expect(failures).toEqual([]);
});

it("keeps independent agent-sidebar accents and VS Code glyphs readable", () => {
  failures.length = 0;
  const sidebar = blocks(readFileSync(new URL("../src/sessionSidebar.css", import.meta.url), "utf8")).dark;
  const vscode = token(dark, "kind-vscode");
  for (const role of ["harness-claude", "harness-codex", "harness-opencode", "accent-pr", "accent-summary", "accent-prompt"]) {
    for (const [name, bg] of Object.entries(backgrounds)) {
      check(`agent sidebar ${role} on ${name}`, parse(token(sidebar, `sb-${role}`)).rgb, bg);
    }
  }
  for (const [name, bg] of Object.entries(backgrounds)) check(`VS Code icon on ${name}`, parse(vscode).rgb, bg);
  expect(failures).toEqual([]);
});

// SGR 40/100 and 47/107 use the same palette entries as foreground SGR
// 30/90 and 37/97. Testing only the default background misses regressions
// such as pale ANSI black washing out explicit white-on-black output.
const explicitBackgroundPairs = [
  ["white", "black", "37;40"],
  ["brightWhite", "black", "97;40"],
  ["white", "brightBlack", "37;100"],
  ["brightWhite", "brightBlack", "97;100"],
  ["black", "white", "30;47"],
  ["black", "brightWhite", "30;107"],
  ["black", "red", "30;41"],
  ["black", "green", "30;42"],
  ["black", "yellow", "30;43"],
  ["black", "blue", "30;44"],
  ["black", "magenta", "30;45"],
  ["black", "cyan", "30;46"],
] as const;

it.each(explicitBackgroundPairs)("keeps ANSI %s on %s readable (SGR %s)", (foreground, background) => {
  const ratio = contrast(parse(DARK_TERMINAL_THEME[foreground]!).rgb, parse(DARK_TERMINAL_THEME[background]!).rgb);
  expect(ratio).toBeGreaterThanOrEqual(4.5);
});
