import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "fs"
import { join } from "path"

const DEBUG_LOG = "/tmp/amf-opencode-sidebar-state.log"
const SIDEBAR_MAX_FILES = 32
const SIDEBAR_RETENTION_MS = 24 * 60 * 60 * 1000
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
    value?.properties?.sessionID ||
    value?.properties?.sessionId ||
    value?.event?.sessionID ||
    value?.event?.sessionId ||
    value?.event?.properties?.sessionID ||
    value?.event?.properties?.sessionId ||
    null
  )
}

function eventPayload(event) {
  return event?.properties || event
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

function extractMessageRole(payload) {
  const role = payload?.message?.role || payload?.role || null
  return typeof role === "string" ? role.toLowerCase() : null
}

function extractSummary(payload) {
  return (
    normalizePrompt(payload?.summary?.title) ||
    normalizePrompt(payload?.message?.summary?.title) ||
    normalizePrompt(payload?.summary?.content) ||
    normalizePrompt(payload?.message?.summary?.content)
  )
}

function normalizeError(value) {
  if (typeof value === "string") {
    const trimmed = value.trim()
    return trimmed.length > 0 ? trimmed : null
  }
  if (value && typeof value === "object") {
    return (
      normalizeError(value.message) ||
      normalizeError(value.error) ||
      normalizeError(value.text) ||
      normalizeError(value.content)
    )
  }
  return null
}

function extractError(payload) {
  return (
    normalizeError(payload?.error) ||
    normalizeError(payload?.result?.error) ||
    normalizeError(payload?.result) ||
    normalizeError(payload?.data?.error) ||
    normalizeError(payload?.message)
  )
}

function extractTodoCount(event) {
  if (typeof event?.count === "number") return event.count
  if (Array.isArray(event?.todos)) return event.todos.length
  if (Array.isArray(event?.items)) return event.items.length
  return null
}

function extractOpenTodoCount(event) {
  const entries = Array.isArray(event?.todos)
    ? event.todos
    : Array.isArray(event?.items)
      ? event.items
      : null
  if (!entries) {
    return extractTodoCount(event)
  }
  return entries.filter((item) => !todoIsClosed(item)).length
}

function todoText(item) {
  const text =
    item?.content ||
    item?.text ||
    item?.title ||
    item?.label ||
    item?.task ||
    item?.name ||
    null
  if (typeof text !== "string") return null
  const trimmed = text.trim()
  return trimmed.length > 0 ? trimmed : null
}

function todoIsClosed(item) {
  if (item?.done === true || item?.completed === true) return true
  const status = (item?.status || item?.state || "").toString().toLowerCase()
  return ["done", "completed", "closed", "cancelled", "canceled"].includes(status)
}

function extractTodoPreview(event) {
  const entries = Array.isArray(event?.todos)
    ? event.todos
    : Array.isArray(event?.items)
      ? event.items
      : []
  if (entries.length === 0) return null

  const openTodos = entries
    .filter((item) => !todoIsClosed(item))
    .map(todoText)
    .filter(Boolean)

  return openTodos.slice(0, 3)
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

function extractSessionStatus(event) {
  const status = event?.status
  if (typeof status === "string") {
    return status
  }
  if (status?.type === "retry" && typeof status?.attempt === "number") {
    return `retry ${status.attempt}`
  }
  if (typeof status?.type === "string") {
    return status.type
  }
  return null
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

function extractNumber(value) {
  const number = Number(value)
  return Number.isFinite(number) ? number : null
}

function extractLspSummary(event) {
  const status = normalizePrompt(
    event?.status ||
    event?.state ||
    event?.health ||
    event?.phase ||
    event?.summary?.status
  )
  const errors =
    extractNumber(event?.errors) ??
    extractNumber(event?.errorCount) ??
    extractNumber(event?.diagnostics?.errors)
  const warnings =
    extractNumber(event?.warnings) ??
    extractNumber(event?.warningCount) ??
    extractNumber(event?.diagnostics?.warnings)
  const servers =
    extractNumber(event?.servers?.length) ??
    extractNumber(event?.serverCount)

  const details = []
  if (errors && errors > 0) {
    details.push(`${errors} error${errors === 1 ? "" : "s"}`)
  }
  if (warnings && warnings > 0) {
    details.push(`${warnings} warning${warnings === 1 ? "" : "s"}`)
  }
  if (details.length > 0 && status) {
    return `${status} · ${details.join(", ")}`
  }
  if (details.length > 0) {
    return details.join(", ")
  }
  if (status) {
    return status
  }
  if (servers && servers > 0) {
    return `${servers} server${servers === 1 ? "" : "s"}`
  }
  return null
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
    todo_preview: state.todoPreview || null,
    pending_permission: state.pendingPermission || null,
    last_error: state.lastError || null,
    lsp_summary: state.lspSummary || null,
    live_summary: state.liveSummary || null,
    additions: state.diff?.additions ?? null,
    deletions: state.diff?.deletions ?? null,
    files: state.diff?.files ?? null,
    updated_at: new Date().toISOString(),
  }
  writeFileSync(join(dir, `${sessionId}.json`), JSON.stringify(payload, null, 2) + "\n")
  pruneSidebarFiles(dir, sessionId)
}

function pruneSidebarFiles(dir, activeSessionId) {
  const staleBefore = Date.now() - SIDEBAR_RETENTION_MS
  const entries = readdirSync(dir)
    .filter((name) => name.endsWith(".json") && name !== `${activeSessionId}.json`)
    .map((name) => {
      const path = join(dir, name)
      let mtimeMs = 0
      try {
        mtimeMs = statSync(path).mtimeMs
      } catch (_) {}
      return { path, mtimeMs }
    })
    .sort((a, b) => b.mtimeMs - a.mtimeMs)

  entries.forEach((entry, index) => {
    if (entry.mtimeMs < staleBefore || index >= SIDEBAR_MAX_FILES - 1) {
      try {
        unlinkSync(entry.path)
      } catch (_) {}
    }
  })
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
    "tool.execute.before": async (input) => {
      const sessionId = sessionIdFrom(input)
      mutateState(directory, sessionId, (state) => {
        state.lastTool =
          input?.tool || input?.toolName || input?.name || input?.tool_name || null
        state.lastError = null
      })
    },
    "tool.execute.after": async (input) => {
      const sessionId = sessionIdFrom(input)
      const lastError = extractError(input)
      mutateState(directory, sessionId, (state) => {
        state.lastTool =
          input?.tool || input?.toolName || input?.name || input?.tool_name || state.lastTool || null
        state.lastError = lastError
      })
    },
    event: async ({ event }) => {
      const payload = eventPayload(event)
      switch (event?.type) {
        case "session.status": {
          const sessionId = sessionIdFrom(payload)
          mutateState(directory, sessionId, (state) => {
            state.status = extractSessionStatus(payload)
          })
          return
        }
        case "session.diff": {
          const sessionId = sessionIdFrom(payload)
          const summary = extractDiffSummary(payload)
          if (!summary) return
          mutateState(directory, sessionId, (state) => {
            state.diff = summary
          })
          return
        }
        case "todo.updated": {
          const sessionId = sessionIdFrom(payload)
          const todoCount = extractOpenTodoCount(payload)
          const todoPreview = extractTodoPreview(payload)
          mutateState(directory, sessionId, (state) => {
            state.todoCount = todoCount
            state.todoPreview = todoPreview
          })
          return
        }
        case "permission.asked": {
          const sessionId = sessionIdFrom(payload)
          mutateState(directory, sessionId, (state) => {
            state.pendingPermission = extractPermission(payload)
          })
          return
        }
        case "permission.replied": {
          const sessionId = sessionIdFrom(payload)
          mutateState(directory, sessionId, (state) => {
            state.pendingPermission = null
          })
          return
        }
        case "message.updated": {
          const message = payload?.info || payload
          const sessionId = sessionIdFrom(message)
          const role = extractMessageRole(message)
          mutateState(directory, sessionId, (state) => {
            if (role === "user") {
              const prompt = extractPrompt(message)
              if (prompt) {
                state.latestPrompt = prompt
              }
              state.liveSummary = null
              return
            }

            if (role !== "assistant") {
              return
            }

            const summary = extractSummary(message)
            if (summary) {
              state.liveSummary = summary
            }
          })
          return
        }
        case "lsp.updated": {
          const sessionId = sessionIdFrom(payload)
          const summary = extractLspSummary(payload)
          mutateState(directory, sessionId, (state) => {
            state.lspSummary = summary
          })
          return
        }
        default:
          return
      }
    },
  }
}
