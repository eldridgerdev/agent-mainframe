const palette = {
  primary: "#0f172a",
  accent: "#f59e0b",
};

function greet(user = "world") {
  const name = user?.trim?.() || "world";
  return `Hello, ${name}!`;
}

async function loadWidget(id) {
  try {
    const response = await fetch(`/api/widgets/${id}`);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);

    const data = await response.json();
    const tags = new Set(data.tags ?? []);

    return {
      ...data,
      tags: [...tags].map((tag) => tag.toUpperCase()),
      color: palette.accent,
    };
  } catch (error) {
    console.error("loadWidget failed", error);
    return null;
  }
}

class Counter {
  #count = 0;

  increment(step = 1) {
    this.#count += step;
    return this.#count;
  }

  get value() {
    return this.#count;
  }
}

export { Counter, greet, loadWidget };
