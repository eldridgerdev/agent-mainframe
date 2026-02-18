import {
  readFileSync,
  writeFileSync,
  existsSync,
  mkdirSync,
  unlinkSync,
} from "fs"
import { join, dirname, relative } from "path"
import { homedir } from "os"
import { randomUUID } from "crypto"

const NOTIFY_DIR = join(homedir(), ".config", "claude-super-vibeless", "notifications")
const SIGNAL_DIR = join(homedir(), ".config", "claude-super-vibeless", "signals")
const RESPONSE_DIR = join(homedir(), ".config", "claude-super-vibeless", "responses")

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
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function getChangeHistoryPath(cwd) {
  return join(cwd, ".amf", "change-history.json")
}

function loadChangeHistory(cwd) {
  const path = getChangeHistoryPath(cwd)
  if (!existsSync(path)) {
    return { version: 1, repo_root: cwd, sessions: [], changes: [], change_sets: [] }
  }
  try {
    return JSON.parse(readFileSync(path, "utf-8"))
  } catch {
    return { version: 1, repo_root: cwd, sessions: [], changes: [], change_sets: [] }
  }
}

function saveChangeHistory(cwd, history) {
  const path = getChangeHistoryPath(cwd)
  const dir = dirname(path)
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true })
  }
  writeFileSync(path, JSON.stringify(history, null, 2) + "\n")
}

function findRevertedChanges(history, filePath, newContent) {
  const reverted = []
  for (const change of history.changes) {
    if (change.file !== filePath || change.reverted) continue
    if (change.old_snippet && newContent.includes(change.old_snippet)) {
      reverted.push(change.id)
    }
  }
  return reverted
}

function markChangesReverted(history, changeIds) {
  for (const change of history.changes) {
    if (changeIds.includes(change.id)) {
      change.reverted = true
    }
  }
}

function addToChangeSet(history, changeId, filePath) {
  const now = new Date().toISOString()
  let openSet = history.change_sets.find((cs) => cs.file === filePath && !cs.finalized_at)
  if (!openSet) {
    openSet = {
      id: randomUUID(),
      change_ids: [],
      file: filePath,
      summary: null,
      created_at: now,
      finalized_at: null,
    }
    history.change_sets.push(openSet)
  }
  if (!openSet.change_ids.includes(changeId)) {
    openSet.change_ids.push(changeId)
  }
}

function generateReason(tool, oldSnippet, newSnippet, filePath) {
  const fileName = filePath.split("/").pop()
  
  if (tool === "write") {
    return `Create or update ${fileName}`
  }
  
  if (!oldSnippet && newSnippet) {
    return `Add new code to ${fileName}`
  }
  
  if (oldSnippet && !newSnippet) {
    return `Remove code from ${fileName}`
  }
  
  const oldLines = oldSnippet.split("\n").length
  const newLines = newSnippet.split("\n").length
  
  if (newLines > oldLines) {
    return `Extend functionality in ${fileName}`
  } else if (newLines < oldLines) {
    return `Simplify/refactor code in ${fileName}`
  }
  
  return `Modify code in ${fileName}`
}

export const ChangeTracker = async ({ $, directory }) => {
  return {
    "tool.execute.before": async (input, output) => {
      const tool = input.tool
      if (tool !== "write" && tool !== "edit") return

      const filePath = output.args?.file_path || output.args?.filePath || ""
      if (!filePath) return

      const sessionId = input.sessionID || "opencode"
      const changeId = randomUUID()
      const relativePath = relative(directory, filePath) || filePath
      const oldSnippet = (output.args?.old_string || "").slice(0, 500)
      const newSnippet = (output.args?.new_string || "").slice(0, 500)
      const content = (output.args?.content || "").slice(0, 2000)

      const reason = generateReason(tool, oldSnippet, newSnippet, relativePath)

      ensureDir(NOTIFY_DIR)
      ensureDir(SIGNAL_DIR)
      ensureDir(RESPONSE_DIR)

      const history = loadChangeHistory(directory)
      if (tool === "write" && content) {
        const reverted = findRevertedChanges(history, relativePath, content)
        if (reverted.length > 0) {
          markChangesReverted(history, reverted)
          saveChangeHistory(directory, history)
        }
      }

      const signalFile = join(SIGNAL_DIR, sessionId + ".proceed")
      const responseFile = join(RESPONSE_DIR, sessionId + ".json")

      const notification = {
        session_id: sessionId,
        cwd: directory,
        type: "change-reason",
        file_path: filePath,
        relative_path: relativePath,
        tool: tool,
        change_id: changeId,
        old_snippet: oldSnippet,
        new_snippet: newSnippet,
        reason: reason,
        content_preview: content ? content.slice(0, 200) : null,
        response_file: responseFile,
        proceed_signal: signalFile,
      }

      const notificationFile = join(NOTIFY_DIR, sessionId + ".json")
      writeFileSync(notificationFile, JSON.stringify(notification, null, 2))

      const timeout = 30000
      const startTime = Date.now()

      while (!existsSync(signalFile)) {
        if (Date.now() - startTime > timeout) {
          safeUnlink(notificationFile)
          recordChange(history, {
            id: changeId,
            timestamp: new Date().toISOString(),
            file: relativePath,
            tool,
            old_snippet: oldSnippet,
            new_snippet: newSnippet,
            reason,
            session_id: sessionId,
          })
          saveChangeHistory(directory, history)
          return
        }
        await sleep(100)
      }

      safeUnlink(signalFile)
      safeUnlink(notificationFile)

      let finalReason = reason
      let proceedWithChange = true
      
      if (existsSync(responseFile)) {
        try {
          const response = JSON.parse(readFileSync(responseFile, "utf-8"))
          if (response.reason) {
            finalReason = response.reason
          }
          if (response.skip === true) {
            finalReason = null
          }
          if (response.reject === true) {
            proceedWithChange = false
          }
        } catch {}
        safeUnlink(responseFile)
      }

      if (proceedWithChange) {
        recordChange(history, {
          id: changeId,
          timestamp: new Date().toISOString(),
          file: relativePath,
          tool,
          old_snippet: oldSnippet,
          new_snippet: newSnippet,
          reason: finalReason,
          session_id: sessionId,
        })
        saveChangeHistory(directory, history)
      } else {
        throw new Error("Change rejected by user")
      }
    },
  }
}

function recordChange(history, change) {
  history.changes.push(change)
  addToChangeSet(history, change.id, change.file)
}
