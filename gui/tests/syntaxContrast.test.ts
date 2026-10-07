import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

// Every syntax role must stay readable (WCAG AA body text, 4.5:1) on the
// backgrounds code is drawn on: the code surface, the added/removed row
// tints, the review selection and the Learning reader's selected line. Reads
// the real stylesheets, so retuning a theme token re-runs this check.
const read = (name: string) => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
const syntaxCss = read("syntax.css");
const styles = read("styles.css");

type Rgb = [number, number, number];

function blocks(css: string): { light: string; dark: string } {
  const dark = css.indexOf("@media (prefers-color-scheme: dark)");
  expect(dark).toBeGreaterThan(0);
  return { light: css.slice(0, dark), dark: css.slice(dark) };
}

function token(block: string, name: string): string {
  const match = block.match(new RegExp(`--${name}:\\s*([^;]+);`));
  expect(match, `--${name}`).toBeTruthy();
  return match![1].trim();
}

function parse(color: string): { rgb: Rgb; alpha: number } {
  const hex = color.match(/^#([0-9a-f]{6})$/i);
  if (hex) return { rgb: [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16)) as Rgb, alpha: 1 };
  const rgba = color.match(/^rgba\(\s*(\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\s*\)$/);
  expect(rgba, color).toBeTruthy();
  return { rgb: [Number(rgba![1]), Number(rgba![2]), Number(rgba![3])], alpha: Number(rgba![4]) };
}

function over(color: string, base: Rgb): Rgb {
  const { rgb, alpha } = parse(color);
  return rgb.map((channel, i) => channel * alpha + base[i] * (1 - alpha)) as Rgb;
}

function luminance(rgb: Rgb): number {
  const [r, g, b] = rgb.map((value) => {
    const channel = value / 255;
    return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

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
