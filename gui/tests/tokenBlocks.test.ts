import { expect, it } from "vitest";
import { readCss } from "./contrast";

// Dark tokens are declared twice per stylesheet: under
// `@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) }`
// for the system setting, and under `:root[data-theme="dark"]` for an explicit
// choice. CSS cannot share one declaration list between the two, so this
// keeps them from drifting apart.
function declarations(css: string, opener: string): string[] {
  const start = css.indexOf(opener);
  expect(start, opener).toBeGreaterThanOrEqual(0);
  const body = css.slice(start + opener.length, css.indexOf("}", start));
  return body.split(";").map((d) => d.replace(/\s+/g, " ").trim()).filter(Boolean).sort();
}

it.each(["styles.css", "sessionSidebar.css", "syntax.css"])("declares the same dark tokens for the system and explicit dark theme in %s", (file) => {
  const css = readCss(file);
  const system = declarations(css, ':root:not([data-theme="light"]) {');
  const explicit = declarations(css, ':root[data-theme="dark"] {');
  expect(system.length).toBeGreaterThan(0);
  expect(explicit).toEqual(system);
});
