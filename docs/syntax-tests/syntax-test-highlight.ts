type JsonPrimitive = string | number | boolean | null;

type JsonValue =
  | JsonPrimitive
  | JsonObject
  | JsonValue[]
  | { readonly [key: string]: JsonValue };

interface JsonObject {
  readonly kind: "object";
  readonly value: Record<string, JsonValue>;
}

enum RenderMode {
  Light = "light",
  Dark = "dark",
  Auto = "auto",
}

namespace SyntaxDemo {
  export type Result<T> = {
    ok: true;
    value: T;
    mode: RenderMode;
  } | {
    ok: false;
    error: Error;
  };

  export const parse = <T>(input: string, fallback: T): Result<T> => {
    try {
      const value = JSON.parse(input) as T;
      return { ok: true, value, mode: RenderMode.Auto };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error : new Error(String(error)) };
    }
  };

  export class Formatter<T extends JsonValue> {
    constructor(private readonly prefix: string) {}

    format(value: T): string {
      return `${this.prefix}:${this.stringify(value)}`;
    }

    private stringify(value: T): string {
      return typeof value === "string" ? value : JSON.stringify(value);
    }
  }
}

const palette = new Map<RenderMode, string>([
  [RenderMode.Light, "#ffffff"],
  [RenderMode.Dark, "#111111"],
  [RenderMode.Auto, "var(--surface)"],
]);

const demo = new SyntaxDemo.Formatter<JsonObject>("json");
const sample = demo.format({
  kind: "object",
  value: {
    title: "syntax test",
    active: true,
    count: 3,
    nested: [{ kind: "object", value: { note: "hello" } }],
  },
});

const parsed = SyntaxDemo.parse<JsonObject>(sample, {
  kind: "object",
  value: {},
});

export { palette, parsed, RenderMode, SyntaxDemo };
