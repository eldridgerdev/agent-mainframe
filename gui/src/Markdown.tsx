import { ReactNode } from "react";

// A deliberately small markdown renderer for plan documents: headings,
// paragraphs, bullet/numbered lists, fenced code, `code` and **bold**.
// It builds React elements (never raw HTML), so model output can't inject
// markup. Anything it doesn't recognise renders as plain text.

function inline(text: string): ReactNode[] {
  const parts: ReactNode[] = [];
  const pattern = /(`[^`]+`|\*\*[^*]+\*\*)/g;
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    const index = match.index ?? 0;
    if (index > last) parts.push(text.slice(last, index));
    const token = match[0];
    parts.push(token.startsWith("`")
      ? <code key={index}>{token.slice(1, -1)}</code>
      : <strong key={index}>{token.slice(2, -2)}</strong>);
    last = index + token.length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

export default function Markdown({ source }: { source: string }) {
  const lines = source.split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const key = blocks.length;
    if (line.trim().startsWith("```")) {
      const code: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i].trim().startsWith("```")) code.push(lines[i++]);
      i += 1;
      blocks.push(<pre key={key}><code>{code.join("\n")}</code></pre>);
      continue;
    }
    const heading = /^(#{1,4})\s+(.*)$/.exec(line);
    if (heading) {
      const Tag = `h${heading[1].length}` as "h1" | "h2" | "h3" | "h4";
      blocks.push(<Tag key={key}>{inline(heading[2])}</Tag>);
      i += 1;
      continue;
    }
    if (/^\s*([-*]|\d+\.)\s+/.test(line)) {
      const ordered = /^\s*\d+\./.test(line);
      const items: string[] = [];
      while (i < lines.length && /^\s*([-*]|\d+\.)\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*([-*]|\d+\.)\s+/, ""));
        i += 1;
      }
      const children = items.map((item, n) => <li key={n}>{inline(item)}</li>);
      blocks.push(ordered ? <ol key={key}>{children}</ol> : <ul key={key}>{children}</ul>);
      continue;
    }
    if (!line.trim()) {
      i += 1;
      continue;
    }
    const paragraph: string[] = [];
    while (
      i < lines.length && lines[i].trim()
      && !/^(#{1,4}\s|\s*([-*]|\d+\.)\s|\s*```)/.test(lines[i])
    ) {
      paragraph.push(lines[i++]);
    }
    blocks.push(<p key={key}>{inline(paragraph.join(" "))}</p>);
  }
  return <div className="doc md">{blocks}</div>;
}
