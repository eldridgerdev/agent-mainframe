import { useEffect, useMemo, useState } from "react";

type Status = "idle" | "loading" | "ready";

interface UserCardProps {
  name: string;
  role?: "admin" | "editor" | "viewer";
  tags: string[];
  lastSeen?: Date | null;
}

function formatLastSeen(lastSeen?: Date | null): string {
  if (!lastSeen) return "never";
  return new Intl.DateTimeFormat("en-US", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(lastSeen);
}

export function UserCard({
  name,
  role = "viewer",
  tags,
  lastSeen,
}: UserCardProps) {
  const [status, setStatus] = useState<Status>("idle");
  const [count, setCount] = useState(0);

  useEffect(() => {
    const timer = window.setTimeout(() => setStatus("ready"), 300);
    setStatus("loading");
    return () => window.clearTimeout(timer);
  }, []);

  const tagSummary = useMemo(() => {
    return tags.length > 0 ? tags.join(" • ") : "no tags";
  }, [tags]);

  return (
    <>
      <section
        className={`user-card user-card--${role}`}
        data-status={status}
        onClick={() => setCount((value) => value + 1)}
      >
        <header>
          <h2>{name}</h2>
          <span>{role.toUpperCase()}</span>
        </header>

        <p>
          Last seen: <time>{formatLastSeen(lastSeen)}</time>
        </p>

        <ul>
          {tags.map((tag) => (
            <li key={tag}>
              <code>{tag}</code>
            </li>
          ))}
        </ul>

        <footer>
          <strong>{tagSummary}</strong>
          <button type="button">Clicked {count} times</button>
        </footer>
      </section>
    </>
  );
}

export default function DemoScreen() {
  return (
    <main>
      <UserCard
        name="Taylor"
        role="admin"
        tags={["tsx", "jsx", "syntax", "test"]}
        lastSeen={new Date()}
      />
    </main>
  );
}
