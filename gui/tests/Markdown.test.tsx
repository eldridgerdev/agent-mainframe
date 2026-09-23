// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render } from "@testing-library/react";
import Markdown from "../src/Markdown";

afterEach(cleanup);

describe("Markdown", () => {
  it("renders plan structure", () => {
    const { container } = render(
      <Markdown source={"# Plan\n\n## Steps\n1. Edit `ui.tsx`\n2. **Test** it\n\n- risk\n\n```\ncode\n```\nplain\ntext"} />,
    );
    expect(container.querySelector("h1")?.textContent).toBe("Plan");
    expect(container.querySelectorAll("ol li")).toHaveLength(2);
    expect(container.querySelector("ol code")?.textContent).toBe("ui.tsx");
    expect(container.querySelector("strong")?.textContent).toBe("Test");
    expect(container.querySelector("ul li")?.textContent).toBe("risk");
    expect(container.querySelector("pre")?.textContent).toBe("code");
    expect(container.querySelector("p")?.textContent).toBe("plain text");
  });

  it("never interprets model output as HTML", () => {
    const { container } = render(<Markdown source={"<img src=x onerror=alert(1)> **<b>hi</b>**"} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("b")).toBeNull();
    expect(container.textContent).toContain("<img src=x onerror=alert(1)>");
  });
});
