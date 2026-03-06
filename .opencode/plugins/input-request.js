import { mkdirSync, writeFileSync, unlinkSync, existsSync, appendFileSync } from "fs"
import { join } from "path"
import { homedir } from "os"
import { execFileSync } from "child_process"

const NOTIFY_DIR = join(
  homedir(),
  ".config",
  "amf",
  "notifications"
)
const DEBUG_LOG = "/tmp/amf-opencode-input-request.log"
const notifiedAt = new Map()
const lastStatusBySession = new Map()

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

function ensureNotifyDir() {
  if (!existsSync(NOTIFY_DIR)) {
    mkdirSync(NOTIFY_DIR, { recursive: true })
  }
}

function writeNotification(sessionId, cwd, message, type = "input-request") {
  debug("writeNotification", { sessionId, cwd, type })
  const payload = {
    session_id: sessionId,
    cwd: cwd,
    message: message || "Input requested",
    type: type,
  }

  try {
    execFileSync("amf", ["notify"], {
      input: JSON.stringify(payload),
      stdio: ["pipe", "ignore", "ignore"],
    })
    notifiedAt.set(sessionId, Date.now())
    debug("notify via amf notify", { sessionId, ok: true })
    return
  } catch (err) {
    debug("notify via amf notify failed", {
      sessionId,
      err: String(err),
    })
  }

  ensureNotifyDir()
  const filePath = join(NOTIFY_DIR, `${sessionId}.json`)
  writeFileSync(filePath, JSON.stringify(payload, null, 2))
  notifiedAt.set(sessionId, Date.now())
  debug("notify via file fallback", { sessionId, filePath })
}

function clearNotification(sessionId) {
  const age = Date.now() - (notifiedAt.get(sessionId) || 0)
  if (age > 0 && age < 1500) {
    debug("skip clear due debounce", { sessionId, age })
    return
  }

  const clearPayload = {
    type: "clear",
    session_id: sessionId,
  }

  try {
    execFileSync("amf", ["notify"], {
      input: JSON.stringify(clearPayload),
      stdio: ["pipe", "ignore", "ignore"],
    })
    debug("clear via amf notify", { sessionId, ok: true })
    return
  } catch (err) {
    debug("clear via amf notify failed", {
      sessionId,
      err: String(err),
    })
  }

  const filePath = join(NOTIFY_DIR, `${sessionId}.json`)
  if (existsSync(filePath)) {
    unlinkSync(filePath)
    debug("clear via file fallback", { sessionId, filePath })
  }
}

export const InputRequestPlugin = async ({ directory }) => {
  debug("plugin loaded", { directory })
  return {
    "tool.execute.before": async (input) => {
      const toolName =
        input.tool ||
        input.toolName ||
        input.name ||
        input.tool_name
      debug("tool.execute.before", {
        toolName,
        keys: Object.keys(input || {}),
        sessionID: input?.sessionID,
        sessionId: input?.sessionId,
      })
      // When question tool is used, the AI is waiting for user input
      if (toolName === "question") {
        const sessionId =
          input.sessionID ||
          input.sessionId ||
          "question"
        writeNotification(sessionId, directory, "User input requested", "input-request")
      }
    },
    "session.status": async ({ event }) => {
      const status = event?.status
      debug("session.status", {
        status,
        sessionID: event?.sessionID,
        sessionId: event?.sessionId,
      })
      const sessionId = event.sessionID || event.sessionId
      if (!sessionId) return

      const prev = lastStatusBySession.get(sessionId) || ""
      lastStatusBySession.set(sessionId, status || "")

      if (status === "busy" || status === "running") {
        clearNotification(sessionId)
        return
      }

      // Also notify when the agent transitions from active work to
      // an idle/completed state, which means it is waiting on user input.
      const wasWorking = prev === "busy" || prev === "running"
      const nowWaiting =
        status === "idle" ||
        status === "done" ||
        status === "completed" ||
        status === "waiting"
      if (wasWorking && nowWaiting) {
        writeNotification(
          sessionId,
          directory,
          "Agent finished and is waiting for input",
          "input-request"
        )
      }
    },
  }
}
