import { mkdirSync, writeFileSync, unlinkSync, existsSync } from "fs"
import { join } from "path"
import { homedir } from "os"

const NOTIFY_DIR = join(
  homedir(),
  ".config",
  "amf",
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

export const InputRequestPlugin = async ({ directory }) => {
  return {
    "tool.execute.before": async (input) => {
      // When question tool is used, the AI is waiting for user input
      if (input.tool === "question") {
        const sessionId = input.sessionID || "question"
        writeNotification(sessionId, directory, "User input requested", "input-request")
      }
    },
    "session.status": async ({ event }) => {
      const sessionId = event.sessionID || event.sessionId
      if (!sessionId) return
      if (event.status === "busy" || event.status === "running") {
        clearNotification(sessionId)
      }
    },
  }
}
