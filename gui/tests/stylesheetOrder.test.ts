import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { cssFiles, FEATURE_STYLESHEETS } from "./contrast";

// The contrast tests read FEATURE_STYLESHEETS as "the app's cascade". Keep it
// in step with main.tsx so a new or reordered stylesheet cannot slip past them.
it("lists every feature stylesheet in the order main.tsx imports it", () => {
  const main = readFileSync(new URL("../src/main.tsx", import.meta.url), "utf8");
  const imported = [...main.matchAll(/import\s+"\.\/(styles\/[\w-]+\.css)";/g)].map((m) => m[1]);
  expect(imported).toEqual(FEATURE_STYLESHEETS);
  expect(cssFiles().filter((f) => f.startsWith("styles/")).sort()).toEqual([...FEATURE_STYLESHEETS].sort());
});
