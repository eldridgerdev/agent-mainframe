// WCAG contrast audit for the AMF GUI, evaluated inside the native webview by
// the GUI driver (`amf-gui-driver.py audit-contrast`). Plain script, no
// imports: it installs `window.__amfContrast` and is safe to re-evaluate.
//
// Text: every visible text node's colour, composited over its ancestors'
// backgrounds and opacity, against WCAG 1.4.3 (4.5:1, or 3:1 for large
// text). Controls: inputs, selects, textareas, `.btn` and `.switch` against
// WCAG 1.4.11 (3:1 for the boundary or fill against what surrounds it).
// Disabled elements are reported but tagged, since WCAG exempts them.
//
// Themes are applied in place the way `gui/src/themes.ts::applyTheme` does
// (tokens as inline custom properties plus `data-theme`), so one page state
// can be measured under every catalog theme without navigating. Transitions
// are frozen while measuring: `getComputedStyle` otherwise reports colours
// part-way through a theme change.
(() => {
  const parse = (s) => {
    const m = s && s.match(/rgba?\(([^)]+)\)/);
    if (!m) return null;
    const p = m[1].split(/[ ,/]+/).filter(Boolean).map(Number);
    return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1];
  };
  const over = (top, bottom) =>
    [0, 1, 2].map((i) => top[i] * top[3] + bottom[i] * (1 - top[3])).concat([1]);
  const luminance = (c) => {
    const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
    return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]);
  };
  const contrast = (a, b) => {
    const x = luminance(a), y = luminance(b);
    return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
  };
  const hex = (c) => "#" + c.slice(0, 3).map((v) => Math.round(v).toString(16).padStart(2, "0")).join("");
  // The colour stops of a background-image's top layer: the one drawn
  // directly behind content (a lower layer, such as a border-box ring under
  // a padding-box fill, is not behind the text).
  const topGradientStops = (image) => {
    if (!/gradient/.test(image || "")) return [];
    let depth = 0, end = image.length;
    for (let i = 0; i < image.length; i++) {
      if (image[i] === "(") depth++;
      else if (image[i] === ")") depth--;
      else if (image[i] === "," && depth === 0) { end = i; break; }
    }
    return (image.slice(0, end).match(/rgba?\([^)]+\)/g) || []).map(parse).filter(Boolean);
  };
  // The solid colour behind `el`: its own and its ancestors' backgrounds,
  // composited bottom-up until an opaque one, plus accumulated opacity. When
  // the nearest painted background is a gradient, its stops are returned too
  // (composited over what lies beneath), so text is judged against each.
  const backdrop = (el) => {
    const layers = []; let opacity = 1; let stops = null;
    for (let n = el; n && n.nodeType === 1; n = n.parentElement) {
      const cs = getComputedStyle(n);
      opacity *= parseFloat(cs.opacity);
      if (stops === null && layers.length === 0) {
        const found = topGradientStops(cs.backgroundImage);
        if (found.length) stops = { at: layers.length, colours: found };
      }
      const c = parse(cs.backgroundColor);
      if (c && c[3] > 0) { layers.push(c); if (c[3] >= 1) break; }
    }
    let base = [255, 255, 255, 1];
    for (let i = layers.length - 1; i >= 0; i--) base = over(layers[i], base);
    const gradient = stops ? stops.colours.map((c) => over(c, base)) : [];
    return { bg: base, opacity, gradient };
  };
  const describe = (el) => {
    const parts = [];
    for (let n = el, d = 0; n && n.nodeType === 1 && d < 3; n = n.parentElement, d++) {
      const cls = [...n.classList].slice(0, 3).join(".");
      const role = n.getAttribute("role");
      parts.unshift(n.tagName.toLowerCase() + (cls ? "." + cls : "") + (role ? `[${role}]` : ""));
    }
    return parts.join(" > ");
  };
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height || r.bottom < 0 || r.right < 0 || r.top > innerHeight || r.left > innerWidth) return false;
    return getComputedStyle(el).visibility !== "hidden";
  };
  const disabled = (el) => !!el.closest(":disabled,[aria-disabled=true]");

  function audit(scope) {
    const root = scope ? document.querySelector(scope) : document.body;
    if (!root) throw new Error(`no element matches ${scope}`);
    const text = {};
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let t; (t = walker.nextNode());) {
      const sample = t.textContent.trim();
      const el = t.parentElement;
      if (!sample || !el || el.closest(".xterm, svg, [aria-hidden=true]") || !visible(el)) continue;
      const cs = getComputedStyle(el);
      const { bg, opacity, gradient } = backdrop(el);
      const fg = parse(cs.color);
      if (!fg) continue;
      const ink = [fg[0], fg[1], fg[2], fg[3] * opacity];
      // Over a gradient, the worst stop decides; otherwise the solid backdrop.
      const behind = gradient.length ? gradient : [bg];
      let ratio = Infinity, worstBg = bg, drawn = over(ink, bg);
      for (const b of behind) {
        const r = contrast(over(ink, b), b);
        if (r < ratio) { ratio = r; worstBg = b; drawn = over(ink, b); }
      }
      const size = parseFloat(cs.fontSize);
      const large = size >= 24 || (parseInt(cs.fontWeight, 10) >= 700 && size >= 18.66);
      if (ratio >= (large ? 3 : 4.5)) continue;
      const key = describe(el);
      const prior = text[key];
      if (!prior || ratio < prior.ratio) {
        text[key] = { ratio: +ratio.toFixed(2), fg: hex(drawn), bg: hex(worstBg), px: size, sample: sample.slice(0, 40),
          disabled: disabled(el), gradient: gradient.length > 0 };
      }
    }
    const controls = {};
    // Native checkboxes and radios are drawn by the platform (their CSS
    // colours don't describe what is painted) and xterm's helper textarea is
    // invisible, so neither is judged. Ghost buttons are borderless by design.
    const selector = "input:not([type=hidden]):not([type=checkbox]):not([type=radio]),select,textarea,.btn,.switch";
    // How visible a box's edge or fill is against what surrounds it.
    const boundary = (box) => {
      const cs = getComputedStyle(box);
      const around = backdrop(box.parentElement).bg;
      const border = parse(cs.borderTopColor);
      const own = parse(cs.backgroundColor);
      const fill = own && own[3] > 0 ? over(own, around) : around;
      const edge = border && parseFloat(cs.borderTopWidth) > 0 ? contrast(over(border, around), around) : 1;
      return { ratio: Math.max(edge, contrast(fill, around)), around, drawn: edge > 1 || (own && own[3] > 0) };
    };
    for (const el of root.querySelectorAll(selector)) {
      if (!visible(el) || el.classList.contains("btn-ghost") || el.closest(".xterm")) continue;
      let { ratio, around, drawn } = boundary(el);
      // A borderless, unfilled field inherits its boundary from the box that
      // draws it (the dashed TODO entry row, the composer frame).
      for (let box = el.parentElement, depth = 0; !drawn && box && depth < 3; box = box.parentElement, depth++) {
        ({ ratio, around, drawn } = boundary(box));
      }
      if (ratio < 3) controls[describe(el)] = { ratio: +ratio.toFixed(2), around: hex(around), disabled: disabled(el) };
    }
    return { text, controls };
  }

  let applied = [];
  function applyTheme(id) {
    const root = document.documentElement;
    for (const key of applied) root.style.removeProperty(`--${key}`);
    applied = [];
    if (id === "light" || id === "dark") { root.dataset.theme = id; return; }
    const theme = (state.catalog?.themes || []).find((t) => t.id === id);
    if (!theme) throw new Error(`unknown theme ${id}`);
    if (theme.mode === "system") delete root.dataset.theme; else root.dataset.theme = theme.mode;
    for (const [key, value] of Object.entries(theme.tokens)) { root.style.setProperty(`--${key}`, value); applied.push(key); }
  }

  // Every theme by default: the two built-in palettes plus each catalog theme
  // that carries tokens ("default" follows the system and has none).
  function themeIds() {
    return ["light", "dark"].concat((state.catalog?.themes || []).filter((t) => Object.keys(t.tokens).length).map((t) => t.id));
  }

  // Measures `scope` under each theme and folds the results per element, so
  // one element failing in ten themes is one finding listing ten ratios.
  function auditThemes(scope, ids) {
    const root = document.documentElement;
    const saved = { theme: root.dataset.theme, style: root.getAttribute("style") };
    const freeze = document.createElement("style");
    freeze.textContent = "*,*::before,*::after{transition:none!important;animation:none!important}";
    document.head.appendChild(freeze);
    const themes = ids && ids.length ? ids : themeIds();
    const findings = {};
    try {
      for (const id of themes) {
        applyTheme(id);
        const result = audit(scope);
        for (const [kind, entries] of Object.entries(result)) {
          for (const [element, f] of Object.entries(entries)) {
            const key = `${kind}:${element}`;
            const entry = findings[key] || (findings[key] = { kind, element, sample: f.sample, disabled: f.disabled, worst: f.ratio, themes: {} });
            entry.themes[id] = f.ratio;
            entry.worst = Math.min(entry.worst, f.ratio);
            entry.disabled = entry.disabled && f.disabled;
          }
        }
      }
    } finally {
      applied.forEach((key) => root.style.removeProperty(`--${key}`));
      applied = [];
      if (saved.style === null) root.removeAttribute("style"); else root.setAttribute("style", saved.style);
      if (saved.theme === undefined) delete root.dataset.theme; else root.dataset.theme = saved.theme;
      freeze.remove();
    }
    const list = Object.values(findings).sort((a, b) =>
      (a.disabled - b.disabled) || (Object.keys(b.themes).length - Object.keys(a.themes).length) || (a.worst - b.worst));
    return { themes, findings: list };
  }

  // The catalog comes from the backend asynchronously; evaluate
  // `loadCatalog()`, wait for `ready()`, then audit.
  const state = window.__amfContrast?.state || { catalog: null, error: null };
  function loadCatalog() {
    state.catalog = null; state.error = null;
    window.__TAURI_INTERNALS__.invoke("theme_catalog")
      .then((c) => { state.catalog = c; })
      .catch((e) => { state.error = String(e); });
    return true;
  }
  const ready = () => !!state.catalog || !!state.error;

  window.__amfContrast = { state, audit, applyTheme, themeIds, auditThemes, loadCatalog, ready };
})();
