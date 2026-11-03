import { expect, it } from "vitest";
import { cssFiles, readCss } from "./contrast";

// An undeclared custom property silently falls back to the inherited value
// (for colors, usually currentColor), so a typo or a token that was never
// added renders as the wrong color with no error. Every `var(--x)` must name a
// property declared in some stylesheet, or carry its own fallback.

function stripComments(css: string) {
  return css.replace(/\/\*[\s\S]*?\*\//g, "");
}

it("declares every custom property a stylesheet reads, or gives it a fallback", () => {
  const sources = cssFiles().map((file) => ({ file, css: stripComments(readCss(file)) }));
  const declared = new Set(sources.flatMap(({ css }) => [...css.matchAll(/(--[\w-]+)\s*:/g)].map((m) => m[1])));
  const offenders = new Set<string>();
  for (const { file, css } of sources) {
    for (const m of css.matchAll(/var\(\s*(--[\w-]+)\s*(,)?/g)) {
      if (!m[2] && !declared.has(m[1])) offenders.add(`${file}: ${m[1]}`);
    }
  }
  expect([...offenders]).toEqual([]);
});

it("defines the status aliases the PR panels use", () => {
  const tokens = readCss("styles.css");
  expect(tokens).toMatch(/--success:\s*var\(--green\)/);
  expect(tokens).toMatch(/--warning:\s*var\(--amber\)/);
});
