import { useState } from "react";
import RevisionedSettingsModal from "./RevisionedSettingsModal";
import { SHARED_WITH_TERMINAL } from "./terminalNote";

interface Settings { idle_minutes: number; unattended_hours: number }

export default function DormancySettingsPanel({ onClose, onSaved }: {
  onClose: () => void; onSaved: () => void;
}) {
  const [idle, setIdle] = useState("");
  const [unattended, setUnattended] = useState("");
  const valid = [idle, unattended].every((value) => /^\d+$/.test(value)
    && Number.isSafeInteger(Number(value)));

  return <RevisionedSettingsModal<Settings> label="Dormancy settings"
    subtitle={`Global settings. ${SHARED_WITH_TERMINAL} Nothing is stopped by saving settings.`}
    loadCommand="dormancy_settings_load" saveCommand="dormancy_settings_save"
    onLoaded={({ settings }) => {
      setIdle(String(settings.idle_minutes));
      setUnattended(String(settings.unattended_hours));
    }}
    draft={valid ? { idle_minutes: Number(idle), unattended_hours: Number(unattended) } : null}
    isDirty={(loaded) => idle !== String(loaded.idle_minutes) || unattended !== String(loaded.unattended_hours)}
    onClose={onClose} onSaved={onSaved}>
    {(busy) => <>
      <label>Idle minutes<input type="number" min="0" step="1" value={idle} disabled={busy}
        onChange={(event) => setIdle(event.target.value)} /></label>
      <label>Unattended hours<input type="number" min="0" step="1" value={unattended} disabled={busy}
        onChange={(event) => setUnattended(event.target.value)} /></label>
      <p className="muted">A feature must exceed both thresholds to appear. Set either value to 0 to turn detection off.
        Saved changes apply here immediately. Restart AMF's terminal app, if it is open, to use the new thresholds there.</p>
      {!valid && <p role="alert">Enter whole numbers of 0 or greater.</p>}
    </>}
  </RevisionedSettingsModal>;
}
