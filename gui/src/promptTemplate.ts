// Mirrors `prompt_library::render_template` / `parse_placeholder_token` so the
// library preview renders without a backend call per keystroke. Insertion still
// resolves on the backend, which remains the authority.

/** The lookup key of the text inside a `{{...}}` token. */
export function placeholderKey(inner: string): string {
  const trimmed = inner.trim();
  const pipe = trimmed.indexOf("|");
  if (pipe < 0) return trimmed;
  const head = trimmed.slice(0, pipe);
  const colon = head.indexOf(":");
  const label = colon < 0 ? "" : head.slice(0, colon).trim();
  return label || trimmed;
}

/** Substitutes each `{{token}}` with its value (or nothing); an unclosed `{{` stays literal. */
export function renderTemplate(body: string, values: [string, string][]): string {
  let out = "";
  let i = 0;
  while (i < body.length) {
    if (body.startsWith("{{", i)) {
      const close = body.indexOf("}}", i + 2);
      if (close >= 0) {
        const key = placeholderKey(body.slice(i + 2, close));
        out += values.find(([candidate]) => candidate === key)?.[1] ?? "";
        i = close + 2;
        continue;
      }
    }
    out += body[i];
    i += 1;
  }
  return out;
}
