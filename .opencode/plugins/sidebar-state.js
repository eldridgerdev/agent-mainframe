import { appendFileSync, existsSync, mkdirSync, writeFileSync } from "fs"
import { join } from "path"

const DEBUG_LOG = "/tmp/amf-opencode-sidebar-state.log"
const stateBySession = new Map()

function debug(message, data) {
  const ts = new Date().toISOString()
  let line = `[${ts}] ${message}`
  if (data !== undefined) {
    try {
      line += ` ${JSON.stringify(data)}`
    } catch (_) {}
  }
  try {
    appendFileSync(DEBUG_LOG, `${line}\n`)
  } catch (_) {}
}

function ensureDir(path) {
  if (!existsSync(path)) {
    mkdirSync(path, { recursive: true })
  }
}

function sidebarDir(directory) {
  return join(directory, ".amf", "opencode-sidebar")
}

function sessionIdFrom(value) {
  return (
    value?.sessionID ||
    value?.sessionId ||
    value?.event?.sessionID ||
    value?.event?.sessionId ||
    null
  )
}

function normalizePrompt(value) {
  if (typeof value === "string") {
    const trimmed = value.trim()
    return trimmed.length > 0 ? trimmed : null
  }
  if (Array.isArray(value)) {
    const text = value
      .map((entry) => {
        if (typeof entry?.text === "string") return entry.text
        if (typeof entry?.content === "string") return entry.content
        return ""
      })
      .filter(Boolean)
      .join("\n")
      .trim()
    return text.length > 0 ? text : null
  }
  return null
}

function extractPrompt(payload) {
  return (
    normalizePrompt(payload?.message?.content) ||
    normalizePrompt(payload?.message?.text) ||
    normalizePrompt(payload?.content) ||
    normalizePrompt(payload?.text) ||
    normalizePrompt(payload?.summary?.title)
  )
}

function extractTodoCount(event) {
  if (typeof event?.count === "number") return event.count
  if (Array.isArray(event?.todos)) return event.todos.length
  if (Array.isArray(event?.items)) return event.items.length
  return null
}

function extractDiffSummary(event) {
  const diff = event?.summary || event?.diff || event
  const additions = Number(diff?.additions ?? diff?.added ?? 0)
  const deletions = Number(diff?.deletions ?? diff?.removed ?? 0)
  const files = Number(diff?.files ?? diff?.fileCount ?? 0)
  if (!Number.isFinite(additions) || !Number.isFinite(deletions) || !Number.isFinite(files)) {
    return null
  }
  if (additions === 0 && deletions === 0 && files === 0) {
    return null
  }
  return { additions, deletions, files }
}

function extractPermission(event) {
  return (
    event?.tool ||
    event?.permission ||
    event?.name ||
    event?.action ||
    "approval requested"
  )
}

function writeSidebarState(directory, sessionId) {
  const state = stateBySession.get(sessionId)
  if (!state) return

  const dir = sidebarDir(directory)
  ensureDir(dir)
  const payload = {
    session_id: sessionId,
    status: state.status || null,
    last_tool: state.lastTool || null,
    latest_prompt: state.latestPrompt || null,
    todo_count: state.todoCount ?? null,
    pending_permission: state.pendingPermission || null,
    additions: state.diff?.additions ?? null,
    deletions: state.diff?.deletions ?? null,
    files: state.diff?.files ?? null,
    updated_at: new Date().toISOString(),
  }
  writeFileSync(join(dir, `${sessionId}.json`), JSON.stringify(payload, null, 2) + "\n")
}

function mutateState(directory, sessionId, updater) {
  if (!sessionId) return
  const current = stateBySession.get(sessionId) || {}
  updater(current)
  stateBySession.set(sessionId, current)
  writeSidebarState(directory, sessionId)
}

export const SidebarStatePlugin = async ({ directory }) => {
  debug("plugin loaded", { directory })
  return {
    "session.status": async ({ event }) => {
      const sessionId = sessionIdFrom(event)
      mutateState(directory, sessionId, (state) => {
        state.status = event?.status || null
      })
    },
    "session.diff": async ({ event }) => {
      const sessionId = sessionIdFrom(event)
      const summary = extractDiffSummary(event)
      if (!summary) return
      mutateState(directory, sessionId, (state) => {
        state.diff = summary
      })
    },
    "todo.updated": async ({ event }) => {
      const sessionId = sessionIdFrom(event)
      const todoCount = extractTodoCount(event)
      mutateState(directory, sessionId, (state) => {
        state.todoCount = todoCount
      })
    },
    "permission.asked": async ({ event }) => {
      const sessionId = sessionIdFrom(event)
      mutateState(directory, sessionId, (state) => {
        state.pendingPermission = extractPermission(event)
      })
    },
    "permission.replied": async ({ event }) => {
      const sessionId = sessionIdFrom(event)
      mutateState(directory, sessionId, (state) => {
        state.pendingPermission = null
      })
    },
    "tool.execute.before": async (input) => {
      const sessionId = sessionIdFrom(input)
      mutateState(directory, sessionId, (state) => {
        state.lastTool =
          input?.tool || input?.toolName || input?.name || input?.tool_name || null
      })
    },
    "tool.execute.after": async (input) => {
      const sessionId = sessionIdFrom(input)
      mutateState(directory, sessionId, (state) => {
        state.lastTool =
          input?.tool || input?.toolName || input?.name || input?.tool_name || state.lastTool || null
      })
    },
    "message.updated": async ({ event }) => {
      const sessionId = sessionIdFrom(event)
      const prompt = extractPrompt(event)
      if (!prompt) return
      mutateState(directory, sessionId, (state) => {
        state.latestPrompt = prompt
      })
    },
  }
}
