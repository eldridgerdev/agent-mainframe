import { readdirSync, readFileSync } from "node:fs";
import { expect, it } from "vitest";

// Colors must be tokens so TUI-derived themes can re-skin them. A hex or
// rgb()/rgba() literal is allowed only inside a rule whose declarations are
// custom properties (the palette blocks), or in the allow-list below.
const dir = new URL("../src/", import.meta.url);

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
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".css"))) {
    for (const { selector, body } of rules(readFileSync(new URL(file, dir), "utf8"))) {
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
