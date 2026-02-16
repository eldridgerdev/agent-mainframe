import { mkdirSync, writeFileSync, unlinkSync, existsSync } from "fs"
import { join } from "path"
import { homedir } from "os"

const NOTIFY_DIR = join(
  homedir(),
  ".config",
  "claude-super-vibeless",
  "notifications"
)

function ensureNotifyDir() {
  if (!existsSync(NOTIFY_DIR)) {
    mkdirSync(NOTIFY_DIR, { recursive: true })
  }
}

function writeNotification(sessionId, cwd, message, type = "input-request") {
  ensureNotifyDir()
  const filePath = join(NOTIFY_DIR, `${sessionId}.json`)
  const payload = {
    session_id: sessionId,
    cwd: cwd,
    message: message || "Input requested",
    type: type,
  }
  writeFileSync(filePath, JSON.stringify(payload, null, 2))
}

function clearNotification(sessionId) {
  const filePath = join(NOTIFY_DIR, `${sessionId}.json`)
  if (existsSync(filePath)) {
    unlinkSync(filePath)
  }
}

export const InputRequest = async ({ directory }) => {
  return {
    "session.idle": async ({ event }) => {
      const sessionId = event.sessionID || event.sessionId
      if (!sessionId) return

      const message = event.message || "Session waiting for input"
      writeNotification(sessionId, directory, message)
    },

    "session.status": async ({ event }) => {
      const sessionId = event.sessionID || event.sessionId
      if (!sessionId) return

      if (event.status === "busy" || event.status === "running") {
        clearNotification(sessionId)
      }
    },

    "session.deleted": async ({ event }) => {
      const sessionId = event.sessionID || event.sessionId
      if (sessionId) {
        clearNotification(sessionId)
      }
    },

    "permission.asked": async ({ event }) => {
      const sessionId = event.sessionID || event.sessionId
      if (!sessionId) return

      const message = event.message || event.permission || "Permission requested"
      writeNotification(sessionId, directory, message, "permission-request")
    },

    "permission.replied": async ({ event }) => {
      const sessionId = event.sessionID || event.sessionId
      if (sessionId) {
        clearNotification(sessionId)
      }
    },
  }
}
