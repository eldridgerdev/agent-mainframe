import { expect, it } from "vitest";
import { renderTemplate } from "../src/promptTemplate";

// Cases mirror `prompt_library::render_template`'s Rust tests and the backend
// `resolves_defaults_explicit_multiline_and_inline_choices…` fixture.
it("substitutes tokens exactly like the backend renderer", () => {
  expect(renderTemplate("Hi {{name}}!", [["name", "Ada"]])).toBe("Hi Ada!");
  expect(renderTemplate("a{{gone}}b", [])).toBe("ab");
  expect(renderTemplate("{{x}}-{{x}}", [["x", "Z"]])).toBe("Z-Z");
  expect(renderTemplate("{{ k }}", [["k", "v"]])).toBe("v");
  expect(renderTemplate("plain text", [])).toBe("plain text");
  expect(renderTemplate("a {{ unclosed", [])).toBe("a {{ unclosed");
  expect(renderTemplate("Deploy to {{env: dev|staging|prod}}", [["env", "prod"]])).toBe("Deploy to prod");
  expect(renderTemplate("Deploy to {{dev|staging|prod}}", [["dev|staging|prod", "staging"]])).toBe("Deploy to staging");
  expect(renderTemplate("{{ : a|b }}", [[": a|b", "a"]])).toBe("a");
  expect(renderTemplate("{{ name }} {{name}} {{notes}} {{env: dev|prod}} {{rust|go}}", [
    ["name", "Ada"], ["notes", "one\ntwo"], ["env", "dev"], ["rust|go", "rust"],
  ])).toBe("Ada Ada one\ntwo dev rust");
  expect(renderTemplate("世界 {{x}} 🎉", [["x", "{{y}}"]])).toBe("世界 {{y}} 🎉");
});
