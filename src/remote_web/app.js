// AMF Remote — the PWA served by `src/remote_server.rs`. Same origin as
// the API, so the server address is never typed or stored: the page talks
// to whichever AMF served it. Wire shapes mirror `RemoteStatusSnapshot`,
// `RemoteAction` and the terminal socket protocol in `src/remote_terminal.rs`.
"use strict";

const CREDENTIAL_KEY = "amf-remote-credential";
const TERM_MODE_KEY = "amf-remote-term-mode";
const TERM_WRAP_KEY = "amf-remote-term-wrap";
const TERM_FONT_KEY = "amf-remote-term-font";
const HISTORY_LINES = 2000;
const POLL_MS = 3000;
const XTERM_VERSION = "5.5.0";
// Subresource Integrity for the two files loaded from the CDN, which share
// an origin with the device token. Checked against the npm tarball for
// XTERM_VERSION — recompute both when bumping it.
const XTERM_JS_SRI = "sha384-M169f14mRZOXm3hD/v2Ti0ThIT/RnAQagXA9nlE15yHAtrW19gdePJh/HaTzUOe/";
const XTERM_CSS_SRI = "sha384-8Xk9wy/gzEDUKrXtrmCFa2bBuK3BpjpDuL/p0SeKQX19Khl/M+lHOgD/CyYf7efP";

const $ = (id) => document.getElementById(id);

// ---- Storage (every access guarded: private windows can throw) ----------

function storageGet(key) {
  try { return localStorage.getItem(key); } catch { return null; }
}
function storageSet(key, value) {
  try { localStorage.setItem(key, value); } catch { /* not kept */ }
}
function storageRemove(key) {
  try { localStorage.removeItem(key); } catch { /* nothing kept */ }
}

function loadCredential() {
  try {
    const raw = storageGet(CREDENTIAL_KEY);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}
const saveCredential = (c) => storageSet(CREDENTIAL_KEY, JSON.stringify(c));
const clearCredential = () => storageRemove(CREDENTIAL_KEY);

// ---- Small helpers -------------------------------------------------------

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

let toastTimer = null;
function toast(message, isError = false) {
  const node = $("toast");
  node.textContent = message;
  node.classList.toggle("error", isError);
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { node.hidden = true; }, isError ? 5000 : 2500);
}

function authHeaders(extra = {}) {
  return { Authorization: `Bearer ${loadCredential()?.token}`, ...extra };
}

async function responseError(response) {
  try {
    return (await response.json()).error ?? `HTTP ${response.status}`;
  } catch {
    return `HTTP ${response.status}`;
  }
}

// Any authenticated call that 401s means the device was revoked (or the
// token is otherwise gone): back to pairing.
function handleUnauthorized(response) {
  if (response.status !== 401) return false;
  clearCredential();
  route();
  return true;
}

async function action(body) {
  const response = await fetch("/actions", {
    method: "POST",
    headers: authHeaders({ "Content-Type": "application/json" }),
    body: JSON.stringify(body),
  });
  if (handleUnauthorized(response)) throw new Error("This device is no longer paired.");
  if (!response.ok) throw new Error(await responseError(response));
  return (await response.json()).message;
}

// ---- Routing -------------------------------------------------------------
//
// #/            home
// #/f/<id>      a feature
// #/s/<id>      a live session
// #/d/<id>      a feature's changes
// #/t/<id>      a feature's TODO lists
// #/new         create a feature

const VIEWS = ["pairing", "home", "feature", "new", "session", "diff", "todos"];

function showView(name, title) {
  for (const view of VIEWS) $(`view-${view}`).hidden = view !== name;
  $("title").textContent = title ?? "AMF Remote";
  $("back").hidden = name === "home" || name === "pairing";
  $("unpair").hidden = name !== "home";
  document.body.classList.toggle("in-session", name === "session");
}

function currentRoute() {
  const [, kind, id] = (location.hash || "#/").split("/");
  return { kind: kind || "", id: id ? decodeURIComponent(id) : "" };
}

function navigate(hash) {
  if (location.hash === hash) route();
  else location.hash = hash;
}

function route() {
  closeSheet();
  if (!loadCredential() || new URLSearchParams(location.search).has("code")) {
    closeTerminal();
    stopPolling();
    showPairing();
    return;
  }
  const { kind, id } = currentRoute();
  if (kind !== "s") closeTerminal();
  if (kind === "f") showFeature(id);
  else if (kind === "s") showSession(id);
  else if (kind === "new") showNewFeature();
  else if (kind === "d") showDiff(id);
  else if (kind === "t") showTodos(id);
  else showHome();
  startPolling();
}

// ---- Status snapshot ----------------------------------------------------

let snapshot = null;
let pollTimer = null;
let lastOk = 0;

function stopPolling() {
  clearTimeout(pollTimer);
  pollTimer = null;
}

function startPolling() {
  if (pollTimer === null) refresh();
}

async function refresh() {
  stopPolling();
  if (!loadCredential()) return;
  try {
    const response = await fetch("/status", { headers: authHeaders(), cache: "no-store" });
    if (handleUnauthorized(response)) return;
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    snapshot = await response.json();
    lastOk = Date.now();
    setConnection(true);
    renderCurrent();
  } catch (e) {
    setConnection(false, e.message);
  }
  // Polling only while visible: a backgrounded tab shouldn't keep the
  // desktop marking this device as seen.
  if (!document.hidden) pollTimer = setTimeout(refresh, POLL_MS);
}

function setConnection(ok, detail) {
  const node = $("conn");
  node.hidden = ok;
  node.classList.toggle("bad", !ok);
  node.textContent = ok ? "" : "offline";
  node.title = detail ?? "";
  if (!ok && !$("view-home").hidden) {
    $("status-meta").textContent = `Can't reach AMF (${detail}) — retrying`;
  }
}

function renderCurrent() {
  const { kind, id } = currentRoute();
  if (!$("view-home").hidden) renderHome();
  else if (!$("view-feature").hidden && kind === "f") renderFeature(id);
  else if (!$("view-new").hidden) renderNewProjects();
  else if (!$("view-session").hidden && kind === "s") renderSessionHeader(id);
}

const features = () => snapshot?.features ?? [];
const findFeature = (id) => features().find((f) => f.feature_id === id);
function findSession(id) {
  for (const feature of features()) {
    const session = (feature.sessions ?? []).find((s) => s.id === id);
    if (session) return { feature, session };
  }
  return null;
}

// The session a feature's row jumps to: its first live agent, else any
// live session.
function primarySession(feature) {
  const live = (feature.sessions ?? []).filter((s) => s.live);
  return live.find((s) => ["claude", "codex", "opencode", "pi"].includes(s.kind)) ?? live[0];
}

// ---- Pairing -------------------------------------------------------------

const PAIRING_ERRORS = {
  401: "Invalid code",
  410: "Code expired — get a new one on the desktop (r)",
  429: "Too many attempts — get a new code on the desktop",
  503: "AMF did not answer — is the pairing dialog still open?",
};

function defaultDeviceName() {
  const ua = navigator.userAgent;
  if (/Android/i.test(ua)) return "Android phone";
  if (/iPhone|iPad/i.test(ua)) return "iPhone";
  return "Browser";
}

function showPairing() {
  showView("pairing");
  // A QR scan lands here as /?code=123456 — prefill it and drop it from
  // the URL so a reload or an installed-app launch doesn't resubmit it.
  const params = new URLSearchParams(location.search);
  const code = params.get("code");
  if (code) {
    $("code").value = code;
    history.replaceState(null, "", "/");
  }
  if (!$("device-name").value) $("device-name").value = defaultDeviceName();
  (code ? $("pair-submit") : $("code")).focus();
}

async function pair(event) {
  event.preventDefault();
  const error = $("pair-error");
  const submit = $("pair-submit");
  error.hidden = true;
  submit.disabled = true;

  try {
    const response = await fetch("/pair/exchange", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        code: $("code").value.trim(),
        device_name: $("device-name").value.trim(),
      }),
    });
    if (!response.ok) {
      error.textContent = PAIRING_ERRORS[response.status]
        ?? `Pairing failed (HTTP ${response.status})`;
      error.hidden = false;
      return;
    }
    const body = await response.json();
    saveCredential({ deviceId: body.device_id, token: body.token });
    $("code").value = "";
    navigate("#/");
  } catch {
    error.textContent = "Could not reach AMF";
    error.hidden = false;
  } finally {
    submit.disabled = false;
  }
}

// ---- Home ----------------------------------------------------------------

function showHome() {
  showView("home");
  refreshPush();
  if (snapshot) renderHome();
}

function featureItem(feature, { showProject, href }) {
  const li = el("li");
  if (feature.needs_attention) li.classList.add("attention");
  if (feature.status !== "stopped") li.classList.add("running");
  li.append(el("div", "item-title", feature.feature_name));
  const sub = [showProject ? feature.project_name : null, feature.status, feature.agent]
    .filter(Boolean).join(" · ");
  li.append(el("div", "item-sub", sub));
  if (feature.needs_attention && feature.attention_reason) {
    li.append(el("div", "item-reason", feature.attention_detail
      ? `${feature.attention_reason}: ${feature.attention_detail}` : feature.attention_reason));
  }
  li.addEventListener("click", () =>
    navigate(href ?? `#/f/${encodeURIComponent(feature.feature_id)}`));
  return li;
}

function renderHome() {
  const all = features();
  const attention = all.filter((f) => f.needs_attention);
  $("status-meta").textContent =
    `${attention.length} need${attention.length === 1 ? "s" : ""} attention · updated ` +
    new Date(lastOk).toLocaleTimeString();

  // Needs-attention rows jump straight to the session to answer in.
  $("attention-heading").hidden = attention.length === 0;
  $("attention").replaceChildren(...attention.map((feature) => {
    const session = primarySession(feature);
    return featureItem(feature, {
      showProject: true,
      href: session ? `#/s/${encodeURIComponent(session.id)}` : undefined,
    });
  }));

  const byProject = new Map();
  for (const project of snapshot?.projects ?? []) byProject.set(project.name, []);
  for (const feature of all) {
    if (!byProject.has(feature.project_name)) byProject.set(feature.project_name, []);
    byProject.get(feature.project_name).push(feature);
  }
  const groups = [];
  for (const [name, list] of byProject) {
    if (list.length === 0) continue;
    groups.push(el("h3", null, name));
    const ul = el("ul", "list");
    ul.append(...list.map((f) => featureItem(f, { showProject: false })));
    groups.push(ul);
  }
  $("projects").replaceChildren(...groups);
  $("status-empty").hidden = all.length > 0;
}

// ---- Feature ------------------------------------------------------------

function showFeature(id) {
  showView("feature", findFeature(id)?.feature_name ?? "Feature");
  $("feature-message").textContent = "";
  renderFeature(id);
}

function renderFeature(id) {
  const feature = findFeature(id);
  if (!feature) {
    if (snapshot) {
      $("title").textContent = "Feature not found";
      $("sessions").replaceChildren();
    }
    return;
  }
  $("title").textContent = feature.feature_name;
  $("feature-project").textContent = `${feature.project_name} · ${feature.agent}`;
  $("feature-branch").textContent = feature.branch;
  $("feature-summary").hidden = !feature.summary;
  $("feature-summary").textContent = feature.summary ?? "";
  const status = $("feature-status");
  status.textContent = feature.status;
  status.className = `badge ${feature.status}`;
  $("feature-attention").textContent = feature.needs_attention
    ? `Needs attention: ${[feature.attention_reason, feature.attention_detail].filter(Boolean).join(" — ")}` : "";
  const running = feature.status !== "stopped";
  $("feature-start").hidden = running;
  $("feature-stop").hidden = !running;

  const sessions = feature.sessions ?? [];
  $("sessions").replaceChildren(...sessions.map((session) => {
    const li = el("li", "with-remove");
    const text = el("div", "grow");
    text.append(el("div", "item-title", session.label || session.kind));
    text.append(el("div", "item-sub", session.live ? session.kind : `${session.kind} · not running`));
    const remove = el("button", "remove", "×");
    remove.setAttribute("aria-label", `Remove ${session.label}`);
    remove.addEventListener("click", (e) => {
      e.stopPropagation();
      if (!confirm(`Remove ${session.label || session.kind}? Its window will be closed.`)) return;
      runFeatureAction(remove, { action: "remove_session", session_id: session.id }, toast);
    });
    li.append(text, remove);
    if (session.live) {
      li.classList.add("running");
      li.addEventListener("click", () => navigate(`#/s/${encodeURIComponent(session.id)}`));
    } else {
      li.classList.add("disabled");
    }
    return li;
  }));
  $("sessions-empty").hidden = sessions.length > 0;
}

async function runFeatureAction(button, body, success) {
  const buttons = document.querySelectorAll("#view-feature button");
  buttons.forEach((b) => { b.disabled = true; });
  $("feature-message").textContent = "Working…";
  try {
    const message = await action(body);
    $("feature-message").textContent = "";
    success?.(message);
    await refresh();
    // The reply can beat the snapshot that reflects it to the server by a
    // tick; look again shortly rather than wait out the poll interval.
    setTimeout(() => { if (!document.hidden) refresh(); }, 800);
  } catch (e) {
    $("feature-message").textContent = e.message;
    toast(e.message, true);
  } finally {
    buttons.forEach((b) => { b.disabled = false; });
  }
}

// ---- Changes ------------------------------------------------------------

async function showDiff(id) {
  const feature = findFeature(id);
  showView("diff", feature ? `${feature.feature_name} · changes` : "Changes");
  $("diff-summary").textContent = "Loading…";
  $("diff-files").replaceChildren();
  try {
    const response = await fetch(`/features/${encodeURIComponent(id)}/diff`, {
      headers: authHeaders(),
      cache: "no-store",
    });
    if (handleUnauthorized(response)) return;
    if (!response.ok) throw new Error(await responseError(response));
    const diff = await response.json();
    if (currentRoute().id !== id) return;
    renderDiff(diff);
  } catch (e) {
    $("diff-summary").textContent = e.message;
  }
}

function renderDiff(diff) {
  const files = diff.files ?? [];
  $("diff-summary").textContent = files.length === 0
    ? `No changes against ${diff.base_ref}.`
    : `${files.length} file${files.length === 1 ? "" : "s"} · +${diff.total_additions} −${diff.total_deletions} against ${diff.base_ref}`;
  $("diff-files").replaceChildren(...files.map((file) => {
    const details = el("details", "diff-file");
    const summary = el("summary");
    const path = el("span", "diff-path mono",
      file.old_path && file.old_path !== file.path ? `${file.old_path} → ${file.path}` : file.path);
    const stat = el("span", "diff-stat");
    stat.append(el("span", "add", `+${file.additions}`), " ", el("span", "del", `−${file.deletions}`));
    summary.append(el("span", "diff-status", file.status), path, stat);
    details.append(summary);
    details.addEventListener("toggle", () => {
      if (!details.open || details.querySelector(".diff-body")) return;
      const body = el("div", "diff-body");
      if (file.is_binary) {
        body.append(el("div", "h", "Binary file"));
      } else if (file.patch == null) {
        body.append(el("div", "h", "Too large to show on the phone."));
      } else {
        for (const line of file.patch.split("\n")) {
          if (/^(diff --git|index |--- |\+\+\+ |new file|deleted file|similarity|rename )/.test(line)) continue;
          const cls = line.startsWith("@@") ? "h" : line.startsWith("+") ? "a" : line.startsWith("-") ? "d" : "";
          body.append(el("div", cls, line || " "));
        }
      }
      details.append(body);
    });
    return details;
  }));
}

// ---- TODOs --------------------------------------------------------------

const TODO_STATUS_LABEL = { not_started: "not started", in_progress: "in progress", completed: "done" };
const TODO_NEXT_STATUS = { not_started: "in_progress", in_progress: "completed", completed: "not_started" };

async function showTodos(id) {
  const feature = findFeature(id);
  showView("todos", feature ? `${feature.feature_name} · TODOs` : "TODOs");
  $("todo-lists").replaceChildren(el("p", "muted", "Loading…"));
  await loadTodos(id);
}

async function loadTodos(id) {
  try {
    const data = await action({ action: "list_todos", feature_id: id });
    if (currentRoute().kind !== "t" || currentRoute().id !== id) return;
    renderTodos(id, data);
  } catch (e) {
    $("todo-lists").replaceChildren(el("p", "error", e.message));
  }
}

async function todoAction(id, body) {
  try {
    await action(body);
  } catch (e) {
    toast(e.message, true);
  }
  await loadTodos(id);
}

function renderTodos(featureId, data) {
  const note = $("todos-note");
  note.hidden = data.editable;
  note.textContent = "TODOs are open on the desk, so they're read-only here until you close them there.";
  $("todo-lists").replaceChildren(...data.lists.map((list) => {
    const section = el("section");
    section.append(el("h2", null, list.label));
    if (list.scratchpad) section.append(el("p", "scratchpad", list.scratchpad));
    if (data.editable) {
      const form = el("form", "todo-add");
      const input = el("input");
      input.placeholder = "Add a TODO…";
      input.enterKeyHint = "done";
      const add = el("button", "small", "Add");
      form.append(input, add);
      form.addEventListener("submit", async (e) => {
        e.preventDefault();
        const title = input.value.trim();
        if (!title) return;
        add.disabled = true;
        await todoAction(featureId, { action: "add_todo", feature_id: featureId, scope: list.scope, title });
      });
      section.append(form);
    }
    const ul = el("ul", "list");
    ul.append(...list.items.map((todo) => {
      const li = el("li", `todo-item${todo.status === "completed" ? " done" : ""}`);
      const check = el("input");
      check.type = "checkbox";
      check.checked = todo.status === "completed";
      check.disabled = !data.editable;
      check.addEventListener("change", () => todoAction(featureId, {
        action: "set_todo_status",
        todo_id: todo.id,
        status: check.checked ? "completed" : "not_started",
      }));
      const text = el("div", "grow");
      text.append(el("div", "item-title", todo.title));
      if (todo.body) text.append(el("div", "item-sub", todo.body));
      const status = el("button", `todo-status ${todo.status}`, TODO_STATUS_LABEL[todo.status] ?? todo.status);
      status.disabled = !data.editable;
      status.title = "Change status";
      status.addEventListener("click", () => todoAction(featureId, {
        action: "set_todo_status", todo_id: todo.id, status: TODO_NEXT_STATUS[todo.status] ?? "not_started",
      }));
      text.append(status);
      li.append(check, text);
      if (data.editable && todo.status !== "completed") {
        const start = el("button", "todo-status", todo.status === "in_progress" ? "Open agent" : "Start agent");
        start.addEventListener("click", async () => {
          start.disabled = true;
          try {
            const result = await action({ action: "start_todo", feature_id: featureId, todo_id: todo.id });
            if (result.prompt) pendingPrompt = { sessionId: result.session_id, text: result.prompt };
            await refresh();
            navigate(`#/s/${encodeURIComponent(result.session_id)}`);
          } catch (e) {
            toast(e.message, true);
            start.disabled = false;
          }
        });
        text.append(" ", start);
      }
      if (data.editable) {
        const remove = el("button", "remove", "×");
        remove.setAttribute("aria-label", `Delete ${todo.title}`);
        remove.addEventListener("click", () => {
          if (confirm(`Delete "${todo.title}"?`)) {
            todoAction(featureId, { action: "delete_todo", todo_id: todo.id });
          }
        });
        li.append(remove);
      }
      return li;
    }));
    if (list.items.length === 0) ul.append(el("p", "muted small-text", "Nothing here."));
    section.append(ul);
    return section;
  }));
}

// ---- New feature --------------------------------------------------------

function showNewFeature() {
  showView("new", "New feature");
  $("new-error").hidden = true;
  renderNewProjects();
}

function renderNewProjects() {
  const select = $("new-project");
  const current = select.value;
  const projects = snapshot?.projects ?? [];
  if (select.options.length === projects.length &&
      [...select.options].every((o, i) => o.value === projects[i].name)) return;
  select.replaceChildren(...projects.map((p) => {
    const option = el("option", null, `${p.name} (${p.preferred_agent})`);
    option.value = p.name;
    return option;
  }));
  if (projects.some((p) => p.name === current)) select.value = current;
}

async function createFeature(event) {
  event.preventDefault();
  const submit = $("new-submit");
  const error = $("new-error");
  error.hidden = true;
  submit.disabled = true;
  submit.textContent = "Creating…";
  try {
    const featureId = await action({
      action: "create_feature",
      project_name: $("new-project").value,
      branch: $("new-branch").value.trim(),
      agent: $("new-agent").value || null,
      mode: $("new-mode").value || null,
      use_worktree: $("new-worktree").value === "" ? null : $("new-worktree").value === "true",
      review: $("new-review").checked,
    });
    $("new-branch").value = "";
    await refresh();
    navigate(featureId ? `#/f/${encodeURIComponent(featureId)}` : "#/");
  } catch (e) {
    error.textContent = e.message;
    error.hidden = false;
  } finally {
    submit.disabled = false;
    submit.textContent = "Create feature";
  }
}

// ---- Session / terminal -------------------------------------------------

let term = null; // { sessionId, socket, retry, gone, frame, xterm, history }
// A prompt to seed a session's input box with when it opens (start_todo).
let pendingPrompt = null;
let ctrlSticky = false;

function termMode() {
  return storageGet(TERM_MODE_KEY) === "full" ? "full" : "simple";
}

function showSession(id) {
  const found = findSession(id);
  showView("session", found ? `${found.feature.feature_name} · ${found.session.label}` : "Session");
  applyTermMode();
  if (term?.sessionId !== id) {
    closeTerminal();
    openTerminal(id);
    action({ action: "session_opened", session_id: id }).catch(() => {});
  }
  if (pendingPrompt?.sessionId === id) {
    const box = $("send-text");
    box.value = pendingPrompt.text;
    pendingPrompt = null;
    autosize(box);
    setTermStatus("Review the prompt below, then Send. The agent may take a moment to start.");
  }
}

function renderSessionHeader(id) {
  const found = findSession(id);
  if (found) $("title").textContent = `${found.feature.feature_name} · ${found.session.label}`;
}

function setTermStatus(text) {
  $("term-status").textContent = text;
}

function openTerminal(sessionId) {
  term = { sessionId, socket: null, retry: null, gone: false, frame: null, xterm: null, history: null };
  setHistoryButton();
  $("screen").textContent = "";
  connectTerminal();
}

function connectTerminal() {
  if (!term || term.gone) return;
  const current = term;
  setTermStatus("Connecting…");
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(
    `${scheme}://${location.host}/sessions/${encodeURIComponent(current.sessionId)}/terminal`,
  );
  current.socket = socket;
  socket.addEventListener("open", () => {
    socket.send(JSON.stringify({ type: "auth", token: loadCredential()?.token }));
    setTermStatus("");
  });
  socket.addEventListener("message", (event) => {
    let message;
    try { message = JSON.parse(event.data); } catch { return; }
    if (message.type === "frame") {
      current.frame = message;
      renderFrame();
    } else if (message.type === "history") {
      current.history = message.ansi;
      setHistoryButton();
      renderFrame();
      const screen = $("screen");
      screen.scrollTop = screen.scrollHeight - screen.clientHeight * 1.5;
    } else if (message.type === "gone") {
      current.gone = true;
      setTermStatus(message.message);
    } else if (message.type === "error") {
      if (message.message === "unauthorized") {
        current.gone = true;
        clearCredential();
        route();
      } else {
        toast(message.message, true);
      }
    }
  });
  socket.addEventListener("close", () => {
    if (term !== current || current.gone) return;
    setTermStatus("Disconnected — reconnecting…");
    current.retry = setTimeout(connectTerminal, 1500);
  });
}

function closeTerminal() {
  if (!term) return;
  const current = term;
  term = null;
  clearTimeout(current.retry);
  current.gone = true;
  current.socket?.close();
  current.xterm?.dispose();
  $("xterm").replaceChildren();
}

function sendTerminal(message) {
  if (term && message.type !== "history" && term.history != null) {
    term.history = null;
    setHistoryButton();
    renderFrame();
  }
  if (term?.socket?.readyState === WebSocket.OPEN) {
    term.socket.send(JSON.stringify(message));
    return true;
  }
  toast("Not connected", true);
  return false;
}

function sendKey(name) {
  sendTerminal({ type: "key", name });
}

function setHistoryButton() {
  $("history-toggle").textContent = term?.history != null ? "Live" : "History";
}

function toggleHistory() {
  if (!term) return;
  if (term.history != null) {
    term.history = null;
    setHistoryButton();
    renderFrame();
    return;
  }
  if (termMode() !== "simple") {
    storageSet(TERM_MODE_KEY, "simple");
    applyTermMode();
  }
  sendTerminal({ type: "history", lines: HISTORY_LINES });
}

function applyFontSize() {
  const size = Number(storageGet(TERM_FONT_KEY)) || 12;
  $("screen").style.fontSize = `${size}px`;
}

function bumpFont(delta) {
  const size = Math.max(7, Math.min(22, (Number(storageGet(TERM_FONT_KEY)) || 12) + delta));
  storageSet(TERM_FONT_KEY, String(size));
  applyFontSize();
}

function applyTermMode() {
  applyFontSize();
  const mode = termMode();
  $("mode-simple").setAttribute("aria-selected", String(mode === "simple"));
  $("mode-full").setAttribute("aria-selected", String(mode === "full"));
  $("screen").hidden = mode !== "simple";
  $("xterm").hidden = mode !== "full";
  $("fit-toggle").hidden = mode !== "simple";
  $("font-down").hidden = mode !== "simple";
  $("font-up").hidden = mode !== "simple";
  const wrap = storageGet(TERM_WRAP_KEY) !== "off";
  $("screen").classList.toggle("wrap", wrap);
  $("fit-toggle").textContent = wrap ? "No wrap" : "Wrap";
  if (mode === "full") ensureXterm().then(renderFrame);
  else renderFrame();
}

function renderFrame() {
  const frame = term?.frame;
  if (!frame) return;
  if (termMode() === "full") {
    if (term.xterm) writeXtermFrame(term.xterm, frame);
    return;
  }
  const screen = $("screen");
  // History is a snapshot, not a stream: while it's open, keep the reader's
  // place instead of re-rendering under them.
  if (term.history != null) {
    if (!screen.dataset.history) {
      screen.innerHTML = ansiToHtml(term.history, null) +
        '<span class="history-mark">— end of history · tap History to go live —</span>';
      screen.dataset.history = "1";
    }
    return;
  }
  delete screen.dataset.history;
  const atBottom = screen.scrollTop + screen.clientHeight >= screen.scrollHeight - 8;
  screen.innerHTML = ansiToHtml(frame.ansi, frame.cursor_visible ? frame : null);
  if (atBottom) screen.scrollTop = screen.scrollHeight;
}

// ---- Prompt library sheet ------------------------------------------------

function closeSheet() {
  $("sheet").hidden = true;
  $("sheet-body").replaceChildren();
}

async function openPrompts() {
  const found = findSession(currentRoute().id);
  if (!found) return;
  $("sheet").hidden = false;
  $("sheet-title").textContent = "Prompts";
  $("sheet-body").replaceChildren(el("p", "muted", "Loading…"));
  try {
    const prompts = await action({ action: "list_prompts", feature_id: found.feature.feature_id });
    if (prompts.length === 0) {
      $("sheet-body").replaceChildren(el("p", "muted",
        "No saved prompts. Add them at the desk (L on the dashboard) or in amf.json."));
      return;
    }
    const ul = el("ul", "list");
    ul.append(...prompts.map((p) => {
      const li = el("li");
      li.append(el("div", "item-title", p.name));
      li.append(el("div", "item-sub", [p.source, p.description].filter(Boolean).join(" · ")));
      li.append(el("div", "prompt-body", p.body));
      li.addEventListener("click", () => choosePrompt(p));
      return li;
    }));
    $("sheet-body").replaceChildren(ul);
  } catch (e) {
    $("sheet-body").replaceChildren(el("p", "error", e.message));
  }
}

function choosePrompt(prompt) {
  if (prompt.slots.length === 0) return insertPrompt(prompt.body, {});
  $("sheet-title").textContent = prompt.name;
  const form = el("form", "form");
  const inputs = prompt.slots.map((slot) => {
    const label = el("label", null, slot.label);
    let input;
    if (slot.kind === "select") {
      input = el("select");
      input.append(...slot.options.map((o) => {
        const option = el("option", null, o);
        option.value = o;
        return option;
      }));
    } else if (slot.kind === "multiline") {
      input = el("textarea");
      input.rows = 3;
    } else {
      input = el("input");
    }
    if (slot.default != null) input.value = slot.default;
    label.append(input);
    form.append(label);
    return [slot.key, input];
  });
  const insert = el("button", null, "Insert");
  insert.type = "submit";
  form.append(insert);
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    insertPrompt(prompt.body, Object.fromEntries(inputs.map(([key, input]) => [key, input.value])));
  });
  $("sheet-body").replaceChildren(form);
  inputs[0]?.[1].focus();
}

async function insertPrompt(body, values) {
  try {
    const text = await action({ action: "render_prompt", body, values });
    const box = $("send-text");
    box.value = box.value ? `${box.value}\n${text}` : text;
    autosize(box);
    closeSheet();
    box.focus();
  } catch (e) {
    toast(e.message, true);
  }
}

// ---- Full view: xterm.js (loaded on first use) ---------------------------

let xtermLoading = null;

function loadScript(src, integrity) {
  return new Promise((resolve, reject) => {
    const script = el("script");
    script.src = src;
    script.integrity = integrity;
    script.crossOrigin = "anonymous";
    script.onload = resolve;
    script.onerror = () => reject(new Error(`Couldn't load ${src}`));
    document.head.append(script);
  });
}

function loadXterm() {
  if (window.Terminal) return Promise.resolve();
  if (!xtermLoading) {
    const base = `https://cdn.jsdelivr.net/npm/@xterm/xterm@${XTERM_VERSION}`;
    const css = el("link");
    css.rel = "stylesheet";
    css.href = `${base}/css/xterm.css`;
    css.integrity = XTERM_CSS_SRI;
    css.crossOrigin = "anonymous";
    document.head.append(css);
    xtermLoading = loadScript(`${base}/lib/xterm.js`, XTERM_JS_SRI).catch((e) => {
      xtermLoading = null;
      throw e;
    });
  }
  return xtermLoading;
}

async function ensureXterm() {
  if (!term || term.xterm) return;
  const current = term;
  try {
    await loadXterm();
  } catch (e) {
    toast(`${e.message} — using the simple view`, true);
    storageSet(TERM_MODE_KEY, "simple");
    applyTermMode();
    return;
  }
  if (term !== current || current.xterm) return;
  const xterm = new window.Terminal({
    cols: current.frame?.cols ?? 80,
    rows: current.frame?.rows ?? 24,
    fontSize: 12,
    scrollback: 0,
    convertEol: false,
    theme: { background: "#0d1117", foreground: "#e6edf3" },
  });
  xterm.open($("xterm"));
  xterm.onData((data) => {
    if (ctrlSticky && data.length === 1 && /[a-z@\[\]\\^_ ]/i.test(data)) {
      setCtrl(false);
      sendKey(`C-${data.toLowerCase() === " " ? "Space" : data.toLowerCase()}`);
      return;
    }
    sendTerminal({ type: "input", data });
  });
  current.xterm = xterm;
}

function fitXterm(xterm, cols) {
  // Scale the font so the desk's column count fits the phone's width;
  // past a readable minimum, scroll sideways instead.
  const width = $("xterm").clientWidth - 4;
  const size = Math.max(7, Math.min(14, Math.floor((width / cols) / 0.6)));
  if (xterm.options.fontSize !== size) xterm.options.fontSize = size;
}

function writeXtermFrame(xterm, frame) {
  if (xterm.cols !== frame.cols || xterm.rows !== frame.rows) {
    xterm.resize(frame.cols, frame.rows);
  }
  fitXterm(xterm, frame.cols);
  const cursor = frame.cursor_visible
    ? `\x1b[${frame.cursor_y + 1};${frame.cursor_x + 1}H\x1b[?25h`
    : "\x1b[?25l";
  xterm.write(`\x1b[?25l\x1b[0m\x1b[H\x1b[2J${frame.ansi.replace(/\n/g, "\r\n")}\x1b[0m${cursor}`);
}

// ---- Simple view: ANSI (SGR only, which is all capture-pane emits) → HTML

const BASE16 = [
  "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
  "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#f0f6fc",
];

function color256(n) {
  if (n < 16) return BASE16[n];
  if (n >= 232) {
    const v = 8 + (n - 232) * 10;
    return `rgb(${v},${v},${v})`;
  }
  const i = n - 16;
  const level = (x) => (x === 0 ? 0 : 55 + x * 40);
  return `rgb(${level(Math.floor(i / 36))},${level(Math.floor(i / 6) % 6)},${level(i % 6)})`;
}

function escapeHtml(text) {
  return text.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
}

function applySgr(style, params) {
  const codes = params === "" ? [0] : params.split(/[;:]/).map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < codes.length; i++) {
    const c = codes[i];
    if (c === 0) Object.assign(style, { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false });
    else if (c === 1) style.bold = true;
    else if (c === 2) style.dim = true;
    else if (c === 3) style.italic = true;
    else if (c === 4) style.underline = true;
    else if (c === 7) style.inverse = true;
    else if (c === 22) { style.bold = false; style.dim = false; }
    else if (c === 23) style.italic = false;
    else if (c === 24) style.underline = false;
    else if (c === 27) style.inverse = false;
    else if (c >= 30 && c <= 37) style.fg = BASE16[c - 30];
    else if (c >= 90 && c <= 97) style.fg = BASE16[c - 90 + 8];
    else if (c === 39) style.fg = null;
    else if (c >= 40 && c <= 47) style.bg = BASE16[c - 40];
    else if (c >= 100 && c <= 107) style.bg = BASE16[c - 100 + 8];
    else if (c === 49) style.bg = null;
    else if (c === 38 || c === 48) {
      const key = c === 38 ? "fg" : "bg";
      if (codes[i + 1] === 5) { style[key] = color256(codes[i + 2] ?? 0); i += 2; }
      else if (codes[i + 1] === 2) {
        style[key] = `rgb(${codes[i + 2] ?? 0},${codes[i + 3] ?? 0},${codes[i + 4] ?? 0})`;
        i += 4;
      }
    }
  }
}

function styleAttr(style) {
  let fg = style.fg;
  let bg = style.bg;
  if (style.inverse) [fg, bg] = [bg ?? "var(--term-bg)", fg ?? "var(--term-fg)"];
  const css = [];
  if (fg) css.push(`color:${fg}`);
  if (bg) css.push(`background:${bg}`);
  if (style.bold) css.push("font-weight:bold");
  if (style.dim) css.push("opacity:.7");
  if (style.italic) css.push("font-style:italic");
  if (style.underline) css.push("text-decoration:underline");
  return css.join(";");
}

function ansiToHtml(ansi, cursor) {
  const style = { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false };
  let html = "";
  let open = "";
  let row = 0;
  let col = 0;
  const emit = (text) => {
    const attr = styleAttr(style);
    if (attr !== open) {
      if (open) html += "</span>";
      if (attr) html += `<span style="${attr}">`;
      open = attr;
    }
    html += text;
  };
  const re = /\x1b\[([0-9;:]*)([A-Za-z])|\x1b.|([^\x1b])/gs;
  let match;
  while ((match = re.exec(ansi)) !== null) {
    if (match[2] !== undefined) {
      if (match[2] === "m") applySgr(style, match[1]);
      continue;
    }
    const ch = match[3];
    if (ch === undefined) continue;
    if (ch === "\n") {
      if (cursor && row === cursor.cursor_y && col <= cursor.cursor_x) {
        emit(" ".repeat(cursor.cursor_x - col) + '<span class="cur"> </span>');
      }
      emit("\n");
      row++;
      col = 0;
      continue;
    }
    const escaped = escapeHtml(ch);
    if (cursor && row === cursor.cursor_y && col === cursor.cursor_x) {
      emit(`<span class="cur">${escaped}</span>`);
    } else {
      emit(escaped);
    }
    col++;
  }
  if (cursor && row === cursor.cursor_y && col <= cursor.cursor_x) {
    emit(" ".repeat(cursor.cursor_x - col) + '<span class="cur"> </span>');
  }
  if (open) html += "</span>";
  return html;
}

function setCtrl(on) {
  ctrlSticky = on;
  $("ctrl-toggle").classList.toggle("on", on);
}

function submitText(event) {
  event?.preventDefault();
  const box = $("send-text");
  const text = box.value;
  if (ctrlSticky && text.length === 1) {
    setCtrl(false);
    sendKey(`C-${text.toLowerCase()}`);
    box.value = "";
    return;
  }
  if (sendTerminal({ type: "text", text, submit: true })) {
    box.value = "";
    autosize(box);
  }
}

function autosize(box) {
  box.style.height = "auto";
  box.style.height = `${Math.min(box.scrollHeight, window.innerHeight * 0.3)}px`;
}

// ---- Push notifications -------------------------------------------------
//
// AMF pushes when a feature starts needing attention (`app/remote_push.rs`).
// Needs a service worker, so a secure context: HTTPS or localhost.

function pushSupported() {
  return window.isSecureContext && "serviceWorker" in navigator &&
    "PushManager" in window && "Notification" in window;
}

function base64UrlToBytes(value) {
  const base64 = (value + "=".repeat((4 - (value.length % 4)) % 4))
    .replace(/-/g, "+").replace(/_/g, "/");
  return Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
}

function bytesToBase64Url(buffer) {
  return btoa(String.fromCharCode(...new Uint8Array(buffer)))
    .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function showPush(state, { enable = false, test = false } = {}) {
  $("push").hidden = false;
  $("push-state").textContent = state;
  $("push-enable").hidden = !enable;
  $("push-test").hidden = !test;
}

// Subscribe (or re-register an existing subscription) with AMF. Re-posting
// on every launch is idempotent server-side, and heals a subscription AMF
// has forgotten — a re-pair, or one dropped after a failed delivery.
async function registerPush({ prompt }) {
  const keyResponse = await fetch("/push/key", { headers: authHeaders() });
  if (handleUnauthorized(keyResponse)) return false;
  if (!keyResponse.ok) throw new Error(await responseError(keyResponse));
  const { public_key: publicKey } = await keyResponse.json();

  const registration = await navigator.serviceWorker.ready;
  let subscription = await registration.pushManager.getSubscription();
  // A subscription made against a different AMF key can't receive this
  // AMF's pushes — replace it.
  const boundKey = subscription?.options?.applicationServerKey;
  if (subscription && boundKey && bytesToBase64Url(boundKey) !== publicKey) {
    await subscription.unsubscribe();
    subscription = null;
  }
  if (!subscription) {
    if (!prompt) return false;
    subscription = await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: base64UrlToBytes(publicKey),
    });
  }

  const response = await fetch("/push/subscribe", {
    method: "POST",
    headers: authHeaders({ "Content-Type": "application/json" }),
    body: JSON.stringify(subscription.toJSON()),
  });
  if (!response.ok) throw new Error(await responseError(response));
  return true;
}

let pushChecked = false;

async function refreshPush() {
  if (pushChecked) return;
  if (!pushSupported()) {
    showPush("Notifications need HTTPS — open AMF over a tunnel (e.g. tailscale serve).");
    return;
  }
  if (Notification.permission === "denied") {
    showPush("Notifications are blocked for this site in your browser settings.");
    return;
  }
  try {
    const registered = Notification.permission === "granted" &&
      await registerPush({ prompt: false });
    pushChecked = true;
    if (registered) {
      showPush("Notifications on", { test: true });
    } else {
      showPush("Get notified when an agent needs you.", { enable: true });
    }
  } catch (e) {
    showPush(`Notifications unavailable: ${e.message}`, { enable: true });
  }
}

async function enablePush() {
  $("push-enable").disabled = true;
  try {
    // Must run inside the tap's user gesture for the browser to prompt.
    const permission = await Notification.requestPermission();
    if (permission !== "granted") {
      pushChecked = false;
      return refreshPush();
    }
    await registerPush({ prompt: true });
    pushChecked = true;
    showPush("Notifications on", { test: true });
  } catch (e) {
    showPush(`Couldn't turn on notifications: ${e.message}`, { enable: true });
  } finally {
    $("push-enable").disabled = false;
  }
}

async function sendTestPush() {
  $("push-test").disabled = true;
  try {
    const response = await fetch("/push/test", { method: "POST", headers: authHeaders() });
    showPush(
      response.ok ? "Test sent — it should arrive in a few seconds." :
        `Test failed: ${await responseError(response)}`,
      { test: true },
    );
  } catch {
    showPush("Could not reach AMF", { test: true });
  } finally {
    $("push-test").disabled = false;
  }
}

async function unsubscribePush() {
  if (!pushSupported()) return;
  try {
    const registration = await navigator.serviceWorker.getRegistration();
    const subscription = await registration?.pushManager.getSubscription();
    // AMF learns about this from the push service on its next send (410)
    // and forgets the subscription then.
    await subscription?.unsubscribe();
  } catch { /* best effort */ }
}

// ---- Wiring --------------------------------------------------------------

$("pair-form").addEventListener("submit", pair);
$("back").addEventListener("click", () => {
  const { kind, id } = currentRoute();
  if (kind === "s") {
    const found = findSession(id);
    navigate(found ? `#/f/${encodeURIComponent(found.feature.feature_id)}` : "#/");
  } else if (kind === "d" || kind === "t") {
    navigate(`#/f/${encodeURIComponent(id)}`);
  } else {
    navigate("#/");
  }
});
$("new-feature").addEventListener("click", () => navigate("#/new"));
$("feature-diff").addEventListener("click", () => {
  navigate(`#/d/${encodeURIComponent(currentRoute().id)}`);
});
$("feature-todos").addEventListener("click", () => {
  navigate(`#/t/${encodeURIComponent(currentRoute().id)}`);
});
$("new-form").addEventListener("submit", createFeature);
$("feature-start").addEventListener("click", (e) => {
  const { id } = currentRoute();
  runFeatureAction(e.target, { action: "start_feature", feature_id: id }, toast);
});
$("feature-stop").addEventListener("click", (e) => {
  const { id } = currentRoute();
  const name = findFeature(id)?.feature_name ?? "this feature";
  if (!confirm(`Stop ${name}? Its tmux session and agents will be killed.`)) return;
  runFeatureAction(e.target, { action: "stop_feature", feature_id: id }, toast);
});
$("feature-delete").addEventListener("click", (e) => {
  const { id } = currentRoute();
  const feature = findFeature(id);
  if (!feature) return;
  const typed = prompt(
    `Delete ${feature.feature_name}? This kills its sessions and removes its worktree ` +
    `(branch ${feature.branch}). Type the feature name to confirm.`);
  if (typed !== feature.feature_name) {
    if (typed !== null) toast("Name didn't match — nothing deleted.", true);
    return;
  }
  runFeatureAction(e.target, { action: "delete_feature", feature_id: id }, (message) => {
    toast(message);
    navigate("#/");
  });
});
document.querySelectorAll("[data-add]").forEach((button) => {
  button.addEventListener("click", () => {
    const { id } = currentRoute();
    runFeatureAction(button, { action: "add_session", feature_id: id, kind: button.dataset.add },
      (sessionId) => { if (sessionId) navigate(`#/s/${encodeURIComponent(sessionId)}`); });
  });
});

$("mode-simple").addEventListener("click", () => { storageSet(TERM_MODE_KEY, "simple"); applyTermMode(); });
$("mode-full").addEventListener("click", () => { storageSet(TERM_MODE_KEY, "full"); applyTermMode(); });
$("fit-toggle").addEventListener("click", () => {
  storageSet(TERM_WRAP_KEY, storageGet(TERM_WRAP_KEY) === "off" ? "on" : "off");
  applyTermMode();
});
$("ctrl-toggle").addEventListener("click", () => setCtrl(!ctrlSticky));
$("history-toggle").addEventListener("click", toggleHistory);
$("prompts-open").addEventListener("click", openPrompts);
$("sheet-close").addEventListener("click", closeSheet);
$("sheet").addEventListener("click", (e) => { if (e.target === $("sheet")) closeSheet(); });
$("font-down").addEventListener("click", () => bumpFont(-1));
$("font-up").addEventListener("click", () => bumpFont(1));
document.querySelectorAll("#keys [data-key]").forEach((button) => {
  button.addEventListener("click", () => {
    const key = button.dataset.key;
    setCtrl(false);
    sendKey(key);
  });
});
document.querySelectorAll("#keys [data-text]").forEach((button) => {
  button.addEventListener("click", () => {
    if (ctrlSticky) {
      setCtrl(false);
      sendKey(`C-${button.dataset.text}`);
    } else {
      sendTerminal({ type: "text", text: button.dataset.text, submit: false });
    }
  });
});
$("send-form").addEventListener("submit", submitText);
$("send-text").addEventListener("input", (e) => autosize(e.target));
$("send-text").addEventListener("keydown", (e) => {
  // Enter sends; Shift+Enter adds a line (for multi-line replies).
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) submitText(e);
});
// Keep the newest output in view as the keyboard opens and shrinks the page.
window.visualViewport?.addEventListener("resize", () => {
  const screen = $("screen");
  screen.scrollTop = screen.scrollHeight;
  if (term?.xterm && term.frame) fitXterm(term.xterm, term.frame.cols);
});

$("push-enable").addEventListener("click", enablePush);
$("push-test").addEventListener("click", sendTestPush);
$("unpair").addEventListener("click", async () => {
  if (!confirm("Unpair this device? You'll need a new code from the desktop.")) return;
  // Forgets the token locally only; revoking it is the desktop's job
  // (pairing dialog → v → d, d).
  await unsubscribePush();
  clearCredential();
  pushChecked = false;
  $("push").hidden = true;
  route();
});

window.addEventListener("hashchange", route);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    stopPolling();
  } else if (loadCredential()) {
    refresh();
    // A socket dropped while backgrounded reconnects on its own; nudge it
    // now instead of waiting out the backoff.
    if (term && !term.gone && term.socket?.readyState !== WebSocket.OPEN) {
      clearTimeout(term.retry);
      connectTerminal();
    }
  }
});

route();

// Service workers need a secure context (HTTPS or localhost). Over plain
// LAN HTTP this is skipped and the page still works — it just isn't
// installable.
if ("serviceWorker" in navigator && window.isSecureContext) {
  navigator.serviceWorker.register("/sw.js").catch(() => {});
}
