import { readFileSync, writeFileSync, existsSync, mkdirSync, unlinkSync } from "fs"
import { join, dirname } from "path"
import { homedir } from "os"
import { fileURLToPath } from "url"

const NOTIFY_DIR = join(homedir(), ".config", "claude-super-vibeless", "notifications")
const SIGNAL_DIR = join(homedir(), ".config", "claude-super-vibeless", "signals")

function ensureDir(dir) {
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true })
  }
}

function safeUnlink(path) {
  try {
    if (existsSync(path)) {
      unlinkSync(path)
    }
  } catch {}
}

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

export const DiffReview = async ({ $, directory }) => {
  const pluginDir = import.meta.dir || dirname(fileURLToPath(import.meta.url))
  const scriptPath = join(pluginDir, "diff-review.sh")

  return {
    "tool.execute.before": async (input, output) => {
      const tool = input.tool
      if (tool !== "write" && tool !== "edit") return

      const filePath = output.args?.file_path || output.args?.filePath || ""
      if (!filePath) return

      const sessionId = input.sessionID || "opencode"
      const signalFile = join(SIGNAL_DIR, sessionId + ".proceed")
      const jsonPayload = JSON.stringify({
        tool,
        file_path: filePath,
        old_string: output.args?.old_string || "",
        new_string: output.args?.new_string || "",
        content: output.args?.content || "",
        cwd: directory,
      })

      ensureDir(NOTIFY_DIR)
      ensureDir(SIGNAL_DIR)

      const notificationFile = join(NOTIFY_DIR, sessionId + ".json")
      const notification = {
        session_id: sessionId,
        cwd: directory,
        message: "Diff review: " + filePath,
        type: "diff-review",
        proceed_signal: signalFile,
      }
      writeFileSync(notificationFile, JSON.stringify(notification, null, 2))

      const timeout = 300000
      const startTime = Date.now()
      while (!existsSync(signalFile)) {
        if (Date.now() - startTime > timeout) {
          safeUnlink(notificationFile)
          throw new Error("Diff review timed out waiting for user")
        }
        await sleep(500)
      }

      safeUnlink(signalFile)
      safeUnlink(notificationFile)

      const tmpFile = "/tmp/opencode-review-input-" + Date.now() + "-" + Math.random().toString(36).slice(2) + ".json"

      try {
        await Bun.write(tmpFile, jsonPayload)

        const result = await $`bash ${scriptPath} ${tmpFile}`
          .env({ ...process.env, OPENCODE_SESSION_ID: sessionId })
          .quiet()
          .nothrow()

        if (result.exitCode === 2) {
          const stderr = result.stderr.toString().trim()
          throw new Error(stderr || "User rejected this change.")
        }
      } finally {
        await $`rm -f ${tmpFile}`.nothrow().quiet()
      }
    },
  }
}
