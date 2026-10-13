#!/usr/bin/env python3
"""Assert the native GUI's agent availability dialog and capture its frames."""

import json
import pathlib
import sys

from gui_frames import GuiFrames

config_path = pathlib.Path(sys.argv[4])
gui = GuiFrames.from_argv()
evaluate, wait, click, capture = gui.evaluate, gui.wait, gui.click, gui.capture


def toggle(label):
    evaluate(f'Array.from(document.querySelectorAll("label")).find(l=>l.textContent.trim()==={json.dumps(label)}).querySelector("input").click()')


def checked():
    return evaluate('Array.from(document.querySelectorAll("input[type=checkbox]")).filter(e=>e.checked).map(e=>e.parentElement.textContent.trim())')


with gui:
    gui.resize(1180, 780)
    click("Agent availability")
    wait('document.querySelectorAll("input[type=checkbox]").length===4 && !document.body.innerText.includes("Loading or saving settings…")')
    assert checked() == ["Claude Code", "Codex"]
    capture("001-agent-settings-loaded.png", "Global agent choices load from the shared config; project overrides and installed availability still apply.", ["Agent availability", "Claude Code", "Codex", "OpenCode", "Pi", "Project allowed-agent settings take precedence."])

    toggle("Pi")
    click("Cancel")
    wait("!!document.querySelector('[role=alertdialog][aria-label=\"Discard unsaved settings\"]')")
    capture("002-agent-discard-guard.png", "Cancel with unsaved agent choices asks before discarding the draft.", ["Discard unsaved settings and close?", "Keep editing", "Discard changes"])
    click("Keep editing")
    saved = json.loads(config_path.read_text())
    saved["future_setting"] = 43
    config_path.write_text(json.dumps(saved))
    click("Save settings")
    wait('document.body.innerText.includes("Global config changed. Reload settings before saving.")')
    assert checked() == ["Claude Code", "Codex", "Pi"]
    capture("003-agent-stale-save-refused.png", "An external config edit refuses the stale save while retaining the agent draft.", ["Global config changed. Reload settings before saving.", "Reload settings", "Save settings"], allow_alert=True)
    assert json.loads(config_path.read_text())["extension"]["allowed_agents"] == ["claude", "codex"]

    click("Reload settings")
    wait("!!document.querySelector('[role=alertdialog][aria-label=\"Discard unsaved settings\"]')")
    click("Discard changes")
    wait('!document.querySelector("[role=alertdialog]") && !document.body.innerText.includes("Loading or saving settings…")')
    assert checked() == ["Claude Code", "Codex"]
    toggle("Claude Code")
    toggle("Pi")
    click("Save settings")
    wait("!document.querySelector('[role=dialog][aria-label=\"Agent availability\"]')")
    saved = json.loads(config_path.read_text())
    assert saved["extension"]["allowed_agents"] == ["codex", "pi"], saved
    assert saved["future_setting"] == 43 and saved["extension"]["future_nested"]["keep"] is True and saved["dormant_idle_minutes"] == 15, saved
    click("Agent availability")
    wait('document.querySelectorAll("input[type=checkbox]").length===4 && !document.body.innerText.includes("Loading or saving settings…")')
    assert checked() == ["Codex", "Pi"]
    capture("004-agent-settings-saved.png", "Reopening verifies the explicitly saved Codex and Pi choices; unrelated config and existing sessions are preserved.", ["Agent availability", "Existing sessions keep running.", "Codex", "Pi"], 'Array.from(document.querySelectorAll("input[type=checkbox]")).map(e=>e.checked).join(",")==="false,true,false,true"')
    evaluate('window.__agentOptions=undefined;window.__TAURI_INTERNALS__.invoke("new_session_options",{target:{project_id:"shot-project",feature_id:"shot-feature"}}).then(v=>window.__agentOptions={ok:v},e=>window.__agentOptions={err:e})')
    wait('window.__agentOptions!==undefined')
    options = evaluate('window.__agentOptions')
    assert "err" not in options, options
    agent_kinds = [choice["kind"] for choice in options["ok"]["builtin"] if choice["kind"] in ["claude", "codex", "opencode", "pi"]]
    assert agent_kinds == ["codex", "pi"], options
    gui.write_notes()
    print("PASS: agent choices load, discard protection, stale refusal, persistence and unrelated config preservation", flush=True)
