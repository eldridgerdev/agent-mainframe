import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { blocks, token, parse, over, contrast, type Rgb } from "./contrast";

// Every syntax role must stay readable (WCAG AA body text, 4.5:1) on the
// backgrounds code is drawn on: the code surface, the added/removed row
// tints, the review selection and the Learning reader's selected line. Reads
// the real stylesheets, so retuning a theme token re-runs this check.
const read = (name: string) => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
const syntaxCss = read("syntax.css");
const styles = read("styles.css");

function rowTint(kind: "added" | "removed"): string {
  const match = styles.match(new RegExp(`\\.diff-${kind}\\s*\\{\\s*background:\\s*([^;]+);`));
  expect(match, kind).toBeTruthy();
  return match![1].trim().replace(/,\s*\./g, ", 0.");
}

const ROLES = ["comment", "keyword", "function", "string", "number", "type", "property", "tag", "accent", "builtin", "parameter", "punctuation"];

for (const mode of ["light", "dark"] as const) {
  it(`keeps every ${mode} syntax role at AA contrast on code backgrounds`, () => {
    const palette = blocks(syntaxCss)[mode];
    const theme = blocks(styles)[mode];
    const surface = parse(token(theme, "surface")).rgb;
    const backgrounds: Record<string, Rgb> = {
      surface,
      "surface-2": parse(token(theme, "surface-2")).rgb,
      added: over(rowTint("added"), surface),
      removed: over(rowTint("removed"), surface),
      selected: over(token(theme, "accent-soft"), surface),
    };
    const failures: string[] = [];
    for (const role of ROLES) {
      const fg = parse(token(palette, `syn-${role}`)).rgb;
      for (const [name, bg] of Object.entries(backgrounds)) {
        const ratio = contrast(fg, bg);
        if (ratio < 4.5) failures.push(`${role} on ${name}: ${ratio.toFixed(2)}`);
      }
    }
    expect(failures).toEqual([]);
  });
}

it("defines a token for every role in both modes", () => {
  const { light, dark } = blocks(syntaxCss);
  for (const role of ROLES) {
    expect(light).toContain(`--syn-${role}:`);
    expect(dark).toContain(`--syn-${role}:`);
  }
});
