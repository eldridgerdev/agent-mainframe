import { readFileSync } from "fs"
import { join, dirname } from "path"
import { fileURLToPath } from "url"

export const DiffReview = async ({ $, directory }) => {
  const pluginDir = import.meta.dir || dirname(fileURLToPath(import.meta.url))
  const scriptPath = join(pluginDir, "diff-review.sh")

  return {
    "tool.execute.before": async (input, output) => {
      const tool = input.tool
      if (tool !== "write" && tool !== "edit") return

      const filePath = output.args?.file_path || output.args?.filePath || ""
      if (!filePath) return

      const jsonPayload = JSON.stringify({
        tool,
        file_path: filePath,
        old_string: output.args?.old_string || "",
        new_string: output.args?.new_string || "",
        content: output.args?.content || "",
        cwd: directory,
      })

      const tmpFile = `/tmp/opencode-review-input-${Date.now()}-${Math.random().toString(36).slice(2)}.json`

      try {
        await Bun.write(tmpFile, jsonPayload)

        const result = await $`bash ${scriptPath} ${tmpFile}`
          .env({ ...process.env, OPENCODE_SESSION_ID: input.sessionID || "opencode" })
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
