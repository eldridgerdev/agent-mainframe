import { useEffect, useRef, useState } from "react";
import { asGuiError } from "./api";
import type { ImageData } from "./screenshotsApi";
import { Spinner } from "./ui";

export function ScreenshotImage({ identity, caption, load, onData }: { identity: string; caption: string; load: () => Promise<ImageData>; onData?: (image: ImageData) => void }) {
  const [data, setData] = useState<ImageData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const loader = useRef(load); loader.current = load;
  const received = useRef(onData); received.current = onData;
  useEffect(() => {
    let live = true; setData(null); setError(null);
    loader.current().then((value) => { if (live) { setData(value); received.current?.(value); } }).catch((err) => { if (live) setError(asGuiError(err).message); });
    return () => { live = false; };
  }, [identity, retry]);
  if (error) return <div role="alert"><p>{error}</p><button className="btn btn-secondary" onClick={() => setRetry((v) => v + 1)}>Retry image</button></div>;
  if (!data) return <span role="status"><Spinner /> Loading image…</span>;
  return <img src={data.data_url} alt={caption} draggable={false} />;
}

export default function ScreenshotViewer({ identity, caption, provenance, index, total, load, onMove, onClose }: {
  identity: string; caption: string; provenance: string[]; index: number; total: number; load: () => Promise<ImageData>; onMove: (offset: number) => void; onClose: () => void;
}) {
  const [scale, setScale] = useState<number | null>(null);
  const [size, setSize] = useState<ImageData | null>(null);
  const canvas = useRef<HTMLDivElement>(null);
  const move = useRef(onMove); move.current = onMove;
  useEffect(() => { setScale(null); setSize(null); if (canvas.current) { canvas.current.scrollLeft = 0; canvas.current.scrollTop = 0; } }, [identity]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => { if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); move.current(event.key === "ArrowLeft" ? -1 : 1); } };
    window.addEventListener("keydown", key); return () => window.removeEventListener("keydown", key);
  }, []);
  const pan = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  return <section aria-label="Screenshot viewer">
    <div className="row screenshot-toolbar">
      <button className="btn btn-secondary" onClick={onClose}>Back to gallery</button>
      <button className="btn btn-secondary" disabled={index <= 0} onClick={() => onMove(-1)}>Previous</button><span>{index + 1} / {total}</span>
      <button className="btn btn-secondary" disabled={index >= total - 1} onClick={() => onMove(1)}>Next</button>
      <button className="btn btn-secondary" onClick={() => setScale(null)}>Fit</button><button className="btn btn-secondary" onClick={() => setScale(1)}>Original size</button>
      <button className="btn btn-secondary" onClick={() => setScale((v) => Math.max(.1, (v ?? 1) / 1.25))}>Zoom out</button><button className="btn btn-secondary" onClick={() => setScale((v) => Math.min(8, (v ?? 1) * 1.25))}>Zoom in</button>
    </div>
    <p>{caption}</p>{provenance.map((label) => <p key={label} className="small muted">{label}</p>)}
    <div ref={canvas} className={`screenshot-canvas ${scale === null ? "screenshot-fit" : "screenshot-original"}`} tabIndex={0} aria-label="Image; drag or scroll to pan"
      onPointerDown={(e) => { if (scale !== null && canvas.current && e.button === 0) { pan.current = { x: e.clientX, y: e.clientY, left: canvas.current.scrollLeft, top: canvas.current.scrollTop }; canvas.current.setPointerCapture?.(e.pointerId); } }}
      onPointerMove={(e) => { if (pan.current && canvas.current) { canvas.current.scrollLeft = pan.current.left - (e.clientX - pan.current.x); canvas.current.scrollTop = pan.current.top - (e.clientY - pan.current.y); } }}
      onPointerUp={() => { pan.current = null; }} onPointerCancel={() => { pan.current = null; }}>
      <div style={scale !== null && size ? { width: size.width * scale, height: size.height * scale } : { width: "100%", height: "100%" }}>
        <ScreenshotImage identity={identity} caption={caption} load={load} onData={setSize} />
      </div>
    </div>
  </section>;
}
