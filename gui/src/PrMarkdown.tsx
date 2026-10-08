import { useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import { asGuiError } from "./api";
import { ImageData, inlinePrImage, openScreenshotBrowser, prDescription } from "./screenshotsApi";
import { Spinner } from "./ui";

export type OpenPrImage = (image: ImageData, caption: string, trigger: HTMLElement) => void;

function InlinePrImage({ workflowId, source, caption, onOpen }: {
  workflowId: string; source: string; caption: string; onOpen: OpenPrImage;
}) {
  const [image, setImage] = useState<ImageData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const host = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(typeof IntersectionObserver === "undefined");
  useEffect(() => {
    if (!host.current || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    }, { rootMargin: "300px" });
    observer.observe(host.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!visible) return;
    let live = true;
    setImage(null);
    setError(null);
    inlinePrImage(workflowId, source)
      .then((value) => { if (live) setImage(value); })
      .catch((err) => { if (live) setError(asGuiError(err).message); });
    return () => { live = false; };
  }, [workflowId, source, retry, visible]);

  return <span ref={host} className="pr-inline-image">
    {image ? <button type="button" className="pr-image-open" aria-label={`Enlarge image: ${caption}`}
      onClick={(event) => { event.preventDefault(); event.stopPropagation(); onOpen(image, caption, event.currentTarget); }}>
      <img src={image.data_url} alt={caption} draggable={false} />
    </button> : error ? <span role="alert">
      {caption}: {error}{" "}
      <button type="button" className="btn btn-secondary btn-sm" onClick={(event) => {
        event.preventDefault(); event.stopPropagation(); setRetry((value) => value + 1);
      }}>Retry image</button>
    </span> : <span role="status"><Spinner /> Loading {caption}…</span>}
  </span>;
}

/** PR Markdown renders GitHub's HTML image form through sanitized nodes. Image
 * bytes always come from the bounded native adapter, never a remote DOM src. */
export default function PrMarkdown({ source, workflowId, identity, onOpenImage }: {
  source: string; workflowId: string; identity: string; onOpenImage: OpenPrImage;
}) {
  const [linkError, setLinkError] = useState<string | null>(null);
  const components = useMemo(() => ({
    img: ({ src, alt }: { src?: string; alt?: string }) => src
      ? <InlinePrImage key={`${identity}:${src}`} workflowId={workflowId} source={src}
        caption={alt || "PR image"} onOpen={onOpenImage} />
      : <span className="muted">{alt || "Image has no supported source"}</span>,
    a: ({ href, children }: { href?: string; children?: React.ReactNode }) => {
      if (!href || !/^https?:\/\//i.test(href)) return <span>{children}</span>;
      return <a href={href} onClick={(event) => {
        event.preventDefault();
        void openScreenshotBrowser(href).catch((err) => setLinkError(asGuiError(err).message));
      }}>{children}</a>;
    },
  }), [identity, workflowId, onOpenImage]);
  return <div className="doc md pr-markdown">
    {linkError && <p role="alert">{linkError}</p>}
    <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeRaw, rehypeSanitize]}
      components={components}>{source}</ReactMarkdown>
  </div>;
}

export function PrDescription({ workflowId, identity, onOpenImage }: {
  workflowId: string; identity: string; onOpenImage: OpenPrImage;
}) {
  const [source, setSource] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let live = true;
    setSource(null);
    setError(null);
    prDescription(workflowId)
      .then((value) => { if (live) setSource(value); })
      .catch((err) => { if (live) setError(asGuiError(err).message); });
    return () => { live = false; };
  }, [workflowId, identity, retry]);
  return <details className="pr-description" open>
    <summary>PR description</summary>
    {error ? <p role="alert">{error}{" "}<button className="btn btn-secondary btn-sm"
      onClick={() => setRetry((value) => value + 1)}>Retry description</button></p>
      : source === null ? <p role="status"><Spinner /> Loading description…</p>
      : source ? <PrMarkdown source={source} workflowId={workflowId} identity={identity} onOpenImage={onOpenImage} />
      : <p className="muted">No description.</p>}
  </details>;
}
