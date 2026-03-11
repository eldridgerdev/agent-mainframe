import {
  readFileSync,
  writeFileSync,
  existsSync,
  mkdirSync,
  unlinkSync,
  appendFileSync,
} from "fs"
import { join, dirname, relative } from "path"
import { homedir } from "os"
import { randomUUID } from "crypto"

const NOTIFY_DIR = join(homedir(), ".config", "amf", "notifications")
const SIGNAL_DIR = join(homedir(), ".config", "amf", "signals")
const RESPONSE_DIR = join(homedir(), ".config", "amf", "responses")
const DEBUG_LOG = "/tmp/amf-opencode-change-tracker.log"

function debug(message, data) {
  const ts = new Date().toISOString()
  let line = `[${ts}] ${message}`
  if (data !== undefined) {
    try {
      line += ` ${JSON.stringify(data)}`
    } catch {}
  }
  try {
    appendFileSync(DEBUG_LOG, `${line}\n`)
  } catch {}
}

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

function getArgs(input, output) {
  return {
    ...(input?.args || {}),
    ...(output?.args || {}),
  }
}

function getArg(args, ...keys) {
  for (const key of keys) {
    const value = args?.[key]
    if (typeof value === "string" && value.length > 0) {
      return value
    }
  }
  return ""
}

function ensureReviewTempDir(sessionId, changeId) {
  const dir = join("/tmp", "amf-opencode-review", sessionId, changeId)
  ensureDir(dir)
  return dir
}

function buildReviewFiles(tool, sessionId, changeId, filePath, args) {
  const oldString = getArg(args, "old_string", "oldString", "search", "searchText")
  const newString = getArg(args, "new_string", "newString", "replace", "replaceText")
  const content = getArg(args, "content", "new_content", "newContent", "text")
  const originalContent = existsSync(filePath) ? readFileSync(filePath, "utf-8") : ""
  const isNewFile = !existsSync(filePath)

  let proposedContent = originalContent
  if (tool === "write") {
    proposedContent = content
  } else if (oldString) {
    proposedContent = originalContent.includes(oldString)
      ? originalContent.replace(oldString, newString)
      : originalContent
  }

  const tempDir = ensureReviewTempDir(sessionId, changeId)
  const originalPath = join(tempDir, "original")
  const proposedPath = join(tempDir, "proposed")
  writeFileSync(originalPath, originalContent)
  writeFileSync(proposedPath, proposedContent)

  return {
    oldString,
    newString,
    content,
    originalPath,
    proposedPath,
    isNewFile,
  }
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
  }
  if (newLines < oldLines) {
    return `Simplify/refactor code in ${fileName}`
  }

  return `Modify code in ${fileName}`
}

export const ChangeTracker = async ({ directory }) => {
  debug("plugin loaded", { directory })
  return {
    "tool.execute.before": async (input, output) => {
      const tool = input.tool
      const args = getArgs(input, output)
      debug("tool.execute.before", {
        tool,
        sessionID: input?.sessionID,
        inputKeys: Object.keys(input || {}),
        outputKeys: Object.keys(output || {}),
        argKeys: Object.keys(args || {}),
        file_path: getArg(args, "file_path", "filePath", "path"),
      })
      if (tool !== "write" && tool !== "edit") return

      const filePath = getArg(args, "file_path", "filePath", "path")
      if (!filePath) return

      const sessionId = input.sessionID || "opencode"
      const changeId = randomUUID()
      const relativePath = relative(directory, filePath) || filePath
      const reviewFiles = buildReviewFiles(tool, sessionId, changeId, filePath, args)
      const oldSnippet = reviewFiles.oldString.slice(0, 500)
      const newSnippet = reviewFiles.newString.slice(0, 500)
      const content = reviewFiles.content.slice(0, 2000)

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

      const signalFile = join(SIGNAL_DIR, `${sessionId}.proceed`)
      const responseFile = join(RESPONSE_DIR, `${sessionId}.json`)
      const notification = {
        session_id: sessionId,
        cwd: directory,
        type: "change-reason",
        file_path: filePath,
        relative_path: relativePath,
        tool,
        change_id: changeId,
        old_snippet: oldSnippet,
        new_snippet: newSnippet,
        reason,
        content_preview: content ? content.slice(0, 200) : null,
        original_file: reviewFiles.originalPath,
        proposed_file: reviewFiles.proposedPath,
        is_new_file: reviewFiles.isNewFile,
        response_file: responseFile,
        proceed_signal: signalFile,
      }

      const notificationFile = join(NOTIFY_DIR, `${sessionId}.json`)
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
