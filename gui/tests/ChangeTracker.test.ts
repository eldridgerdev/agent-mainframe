// @vitest-environment node
import { expect, it, vi } from "vitest";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";

const sandbox = vi.hoisted(() => ({ home: "" }));
vi.mock("os", async (importOriginal) => ({ ...await importOriginal<typeof import("node:os")>(), homedir: () => sandbox.home }));

it("owns reply paths per write, cleans up, and cannot release a later write with a late signal", async () => {
  sandbox.home = mkdtempSync(join(tmpdir(), "amf-tracker-test-"));
  const before = process.env.AMF_ACTIVE;
  process.env.AMF_ACTIVE = "1";
  const requests: { response_file: string; proceed_signal: string; change_id: string }[] = [];
  const waits: Promise<unknown>[] = [];
  try {
    const { ChangeTracker } = await import("../../.opencode/plugins/change-tracker.js");
    const directory = join(sandbox.home, "repo");
    mkdirSync(directory);
    const source = join(directory, "a.txt");
    writeFileSync(source, "old");
    const tracker = await ChangeTracker({ directory });
    const notifications = join(sandbox.home, ".config/amf/notifications");
    const input = { tool: "edit", sessionID: `test-${process.pid}`, args: { file_path: source, old_string: "old", new_string: "new" } };
    const first = tracker["tool.execute.before"](input, {});
    waits.push(first.catch(() => {}));
    const request = JSON.parse(readFileSync(join(notifications, readdirSync(notifications)[0]), "utf8"));
    requests.push(request);
    const lease = JSON.parse(readFileSync(join(dirname(request.response_file), "waiter.json"), "utf8"));
    expect(lease).toEqual({ pid: process.pid, session_id: input.sessionID, change_id: request.change_id });
    writeFileSync(request.response_file, JSON.stringify({ reject: true }));
    writeFileSync(request.proceed_signal, "");
    await expect(first).rejects.toThrow("Change rejected by user");
    expect(existsSync(dirname(request.response_file))).toBe(false);
    expect(readdirSync(notifications)).toHaveLength(0);

    let settled = false;
    const second = tracker["tool.execute.before"](input, {}).then(() => { settled = true; });
    waits.push(second);
    const next = JSON.parse(readFileSync(join(notifications, readdirSync(notifications)[0]), "utf8"));
    requests.push(next);
    expect(next.proceed_signal).not.toBe(request.proceed_signal);
    expect(next.response_file).not.toBe(request.response_file);
    mkdirSync(dirname(request.proceed_signal), { recursive: true });
    writeFileSync(request.proceed_signal, "");
    await new Promise((resolve) => setTimeout(resolve, 150));
    expect(settled).toBe(false);
    writeFileSync(next.response_file, JSON.stringify({ reject: false, reason: "Reviewed" }));
    writeFileSync(next.proceed_signal, "");
    await second;
    expect(existsSync(dirname(next.response_file))).toBe(false);
    expect(readdirSync(notifications)).toHaveLength(0);
    expect(JSON.parse(readFileSync(join(directory, ".amf/change-history.json"), "utf8")).changes[0].reason).toBe("Reviewed");
  } finally {
    for (const request of requests) {
      if (existsSync(dirname(request.response_file))) {
        writeFileSync(request.response_file, JSON.stringify({ skip: true }));
        writeFileSync(request.proceed_signal, "");
      }
    }
    await Promise.allSettled(waits);
    requests.forEach((request) => rmSync(dirname(request.response_file), { recursive: true, force: true }));
    rmSync(sandbox.home, { recursive: true, force: true });
    if (before === undefined) delete process.env.AMF_ACTIVE;
    else process.env.AMF_ACTIVE = before;
  }
});
