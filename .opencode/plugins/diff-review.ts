import { execFileSync } from "node:child_process"
import { existsSync, mkdirSync, unlinkSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import type { Plugin } from "@opencode-ai/plugin"

const NOTIFY_DIR = join(homedir(), ".config", "claude-super-vibeless", "notifications")
const SIGNAL_DIR = join(homedir(), ".config", "claude-super-vibeless", "signals")

function ensureDir(dir: string) {
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true })
  }
}

function safeUnlink(path: string) {
  try {
    if (existsSync(path)) {
      unlinkSync(path)
    }
  } catch {}
}

function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function isAmfManaged(): boolean {
  if (!process.env.TMUX) return false
  try {
    const sessionName = execFileSync("tmux", ["display-message", "-p", "#S"], {
      encoding: "utf8",
    }).trim()
    return sessionName.startsWith("amf-")
  } catch {
    return false
  }
}

export const DiffReview: Plugin = async ({ $, directory }) => {
  const pluginDir = import.meta.dir || dirname(fileURLToPath(import.meta.url))
  const scriptPath = join(pluginDir, "diff-review.sh")

  return {
    "tool.execute.before": async (input, output) => {
      const tool = input.tool
      if (tool !== "write" && tool !== "edit") return

      const filePath = output.args?.file_path || output.args?.filePath || ""
      if (!filePath) return

      const sessionId = input.sessionID || "opencode"
      const jsonPayload = JSON.stringify({
        tool,
        file_path: filePath,
        old_string: output.args?.old_string || output.args?.oldString || "",
        new_string: output.args?.new_string || output.args?.newString || "",
        content: output.args?.content || "",
        cwd: directory,
      })

      if (isAmfManaged()) {
        const signalFile = join(SIGNAL_DIR, sessionId + ".proceed")
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
      }

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
