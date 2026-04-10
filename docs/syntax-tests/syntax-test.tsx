import { useEffect, useState } from "react";

type Status = "idle" | "loading" | "ready" | "error";

type SyntaxPreviewProps = {
  title: string;
  items: string[];
};

export function SyntaxPreview({ title, items }: SyntaxPreviewProps) {
  const [status, setStatus] = useState<Status>("idle");

  useEffect(() => {
    setStatus("ready");
  }, []);

  return (
    <section className="syntax-preview" data-status={status}>
      <header>
        <h2>{title}</h2>
        <button
          type="button"
          onClick={() => setStatus((current) => (current === "ready" ? "loading" : "ready"))}
        >
          Toggle status
        </button>
      </header>

      {items.length > 0 ? (
        <ul>
          {items.map((item, index) => (
            <li key={`${item}-${index}`}>
              <strong>{index + 1}.</strong> {item}
            </li>
          ))}
        </ul>
      ) : (
        <p>No items available.</p>
      )}
    </section>
  );
}
