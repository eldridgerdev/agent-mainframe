import { expect, it } from "vitest";
import { cssFiles, readCss } from "./contrast";

// Colors must be tokens so TUI-derived themes can re-skin them. A hex or
// rgb()/rgba() literal is allowed only inside a rule whose declarations are
// custom properties (the palette blocks), or in the allow-list below.

type Allow = { selector: RegExp; property: RegExp; why: string };
const ALLOWED: Allow[] = [
  { selector: /.*/, property: /^(box-shadow|--shadow-[\w-]+|--backdrop)$/, why: "shadows and scrims are opaque-black by design" },
  { selector: /^\.screenshot-canvas$/, property: /^background$/, why: "backdrop behind arbitrary screenshot images, not themed" },
  { selector: /^\.tree-chip-mode\.mode-supervibe$/, property: /^background$/, why: "decorative rainbow ring stops" },
  { selector: /^\.switch::after$/, property: /^background$/, why: "toggle knob; dark rules pin it to --accent-fg" },
  { selector: /^\.diff-(added|removed)\b/, property: /^background(-color)?$/, why: "row tints parsed by syntaxContrast.test.ts" },
];

function stripComments(css: string) {
  return css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
}

/** Flat list of {selector, declarations} for every rule, including those nested in @media. */
function rules(css: string) {
  const out: { selector: string; body: string }[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  for (let m; (m = re.exec(stripComments(css))); ) {
    const selector = m[1].trim();
    if (selector.startsWith("@font-face") || selector.startsWith("@keyframes")) continue;
    out.push({ selector, body: m[2] });
  }
  return out;
}

const literal = /#[0-9a-fA-F]{3,8}\b|rgba?\(/;

it("keeps hex and rgb literals inside token blocks or the explicit allow-list", () => {
  const offenders: string[] = [];
  for (const file of cssFiles()) {
    for (const { selector, body } of rules(readCss(file))) {
      const decls = body.split(";").map((d) => d.trim()).filter(Boolean);
      const props = decls.map((d) => d.slice(0, d.indexOf(":")).trim());
      if (props.every((p) => p.startsWith("--") || p === "color-scheme")) continue; // token block
      for (const decl of decls) {
        if (!literal.test(decl)) continue;
        const prop = decl.slice(0, decl.indexOf(":")).trim();
        const ok = selector.split(",").every((sel) =>
          ALLOWED.some((a) => a.selector.test(sel.trim().replace(/^:root(:not\(\[data-theme="light"\]\)|\[data-theme="dark"\])\s*/, "")) && a.property.test(prop)));
        if (!ok) offenders.push(`${file}: ${selector} { ${decl} }`);
      }
    }
  }
  expect(offenders).toEqual([]);
});

/** Every `prop: value` declaration outside :root token blocks, for the given property. */
function declarationsOf(property: RegExp) {
  const out: { where: string; value: string }[] = [];
  for (const file of cssFiles()) {
    for (const { selector, body } of rules(readCss(file))) {
      for (const decl of body.split(";")) {
        const colon = decl.indexOf(":");
        if (colon < 0 || !property.test(decl.slice(0, colon).trim())) continue;
        const value = decl.slice(colon + 1).trim();
        out.push({ where: `${file}: ${selector} { ${decl.trim()} }`, value });
      }
    }
  }
  return out;
}

// Sizes come from the type scale, radius and weight tokens in styles.css so
// they stay consistent; a new raw literal is a new size to reconcile.
it("draws font sizes from the --fs-* scale", () => {
  const offenders = declarationsOf(/^font-size$/)
    .filter(({ value }) => /\d(px|pt)\b/.test(value.replace(/var\(--fs-[\w-]+\)/g, "")))
    .map(({ where }) => where);
  expect(offenders).toEqual([]);
});

it("keeps the smallest type-scale step at 11px or larger", () => {
  const scale = [...readCss("styles.css").matchAll(/--fs-[\w-]+:\s*([\d.]+)px/g)].map((m) => Number(m[1]));
  expect(scale.length).toBeGreaterThan(0);
  expect(Math.min(...scale)).toBeGreaterThanOrEqual(11);
});

it("draws corner radii from the --r-* tokens", () => {
  const offenders = declarationsOf(/^border(-(top|bottom)-(left|right))?-radius$/)
    .filter(({ value }) => /[1-9]\d*(\.\d+)?px\b/.test(value.replace(/var\(--r-[\w-]+\)/g, "")))
    .map(({ where }) => where);
  expect(offenders).toEqual([]);
});

it("uses only the 400, 500 and 600 font weights", () => {
  const offenders = declarationsOf(/^font-weight$/)
    .filter(({ value }) => !["400", "500", "600", "normal", "inherit"].includes(value))
    .map(({ where }) => where);
  expect(offenders).toEqual([]);
});
