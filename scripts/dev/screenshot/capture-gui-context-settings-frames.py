#!/usr/bin/env python3
"""Assert the native GUI's context settings dialog and capture its frames."""

import json
import pathlib
import sys

from gui_frames import GuiFrames

config_path = pathlib.Path(sys.argv[4])
gui = GuiFrames.from_argv()
evaluate, wait, click, capture = gui.evaluate, gui.wait, gui.click, gui.capture


def fill(label, value):
    evaluate(f"(()=>{{const e=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)}).querySelector('input');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));}})()")
    wait(f'Array.from(document.querySelectorAll("label")).find(l=>l.querySelector("span")?.textContent==={json.dumps(label)}).querySelector("input").value==={json.dumps(value)}')


def values():
    return evaluate('Array.from(document.querySelectorAll(".modal input")).map(e=>e.value)')


with gui:
    gui.resize(1180, 840)
    click("Context settings")
    wait('!!document.querySelector(".modal input") && !document.body.innerText.includes("Loading or saving settings…")')
    assert values() == ["500000", "70", "85"]
    capture("001-context-settings-loaded.png", "Loaded global context limit and warning/critical thresholds share the TUI settings; a blank token limit restores harness defaults.", ["Context settings", "Context window (tokens)", "Warning percent", "Critical percent", "Leave blank to restore harness defaults."])
    fill("Warning percent", "90")
    wait('!!document.querySelector("[role=alert]")')
    capture("002-context-invalid-order.png", "Critical must exceed warning; the invalid order blocks Save without writing configuration.", ["Percentages must be 1–100, with critical greater than warning."], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Save settings").disabled', allow_alert=True)
    assert json.loads(config_path.read_text())["context_warning_percent"] == 70
    fill("Warning percent", "70")
    fill("Context window (tokens)", "18446744073709551615")
    click("Cancel")
    wait('!!document.querySelector("[role=alertdialog]")')
    capture("003-context-discard-guard.png", "Cancel protects the unsaved full-width token-limit draft until explicit discard.", ["Discard unsaved settings and close?", "Keep editing", "Discard changes"])
    click("Keep editing")
    saved = json.loads(config_path.read_text())
    saved["future_setting"] = 43
    saved["context_window_override"] = 300000
    saved["context_warning_percent"] = 60
    saved["context_critical_percent"] = 90
    config_path.write_text(json.dumps(saved))
    click("Save settings")
    wait('document.body.innerText.includes("Global config changed. Reload settings before saving.")')
    assert values() == ["18446744073709551615", "70", "85"]
    capture("004-context-stale-save-refused.png", "An external edit refuses a stale save and retains the complete token-limit draft.", ["Global config changed. Reload settings before saving.", "Reload settings"], allow_alert=True)
    assert json.loads(config_path.read_text())["context_window_override"] == 300000
    click("Reload settings")
    wait('!!document.querySelector("[role=alertdialog]")')
    click("Discard changes")
    wait('!document.querySelector("[role=alertdialog]") && !document.body.innerText.includes("Loading or saving settings…")')
    assert values() == ["300000", "60", "90"]
    fill("Context window (tokens)", "18446744073709551615")
    click("Save settings")
    wait('!document.querySelector(".modal")')
    assert json.loads(config_path.read_text())["context_window_override"] == 18446744073709551615
    click("Context settings")
    wait('!!document.querySelector(".modal input") && !document.body.innerText.includes("Loading or saving settings…")')
    assert values() == ["18446744073709551615", "60", "90"]
    capture("005-context-full-u64-saved.png", "Saving and reopening retains the maximum u64 token limit exactly across native IPC and JSON persistence.", ["Context settings", "Context window (tokens)"])
    fill("Context window (tokens)", "")
    fill("Warning percent", "80")
    fill("Critical percent", "95")
    click("Save settings")
    wait('!document.querySelector(".modal")')
    saved = json.loads(config_path.read_text())
    assert "context_window_override" not in saved and saved["context_warning_percent"] == 80 and saved["context_critical_percent"] == 95, saved
    assert saved["future_setting"] == 43 and saved["extension"]["future_nested"]["keep"] is True and saved["dormant_idle_minutes"] == 15, saved
    click("Context settings")
    wait('!!document.querySelector(".modal input") && !document.body.innerText.includes("Loading or saving settings…")')
    assert values() == ["", "80", "95"]
    capture("006-context-defaults-restored.png", "A blank override restores harness defaults while new thresholds, unrelated config and existing sessions are preserved.", ["Context settings", "Leave blank to restore harness defaults.", "Existing sessions keep running."])
    gui.write_notes()
    print("PASS: context loading, ordered validation, unsaved guard, stale refusal, exact u64 save and default restoration", flush=True)
