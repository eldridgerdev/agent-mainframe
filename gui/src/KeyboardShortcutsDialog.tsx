import { Fragment, useState } from "react";
import {
  SCOPES, SHORTCUT_GROUPS, SHORTCUTS, chordKeys, formatBinding, isMacPlatform, parseBinding,
  type ShortcutDef,
} from "./shortcuts";
import { FilterField, Modal } from "./ui";

/** Whether an entry matches every word of the filter, by its action, group,
 *  where it works, or its keys as displayed. */
export function shortcutMatches(def: ShortcutDef, filter: string, mac = isMacPlatform()): boolean {
  const haystack = [def.label, def.group, SCOPES[def.scope].when, ...def.keys.map((keys) => formatBinding(keys, mac))]
    .join(" ").toLowerCase();
  return filter.toLowerCase().split(/\s+/).filter(Boolean).every((word) => haystack.includes(word));
}

function Binding({ binding, mac }: { binding: string; mac: boolean }) {
  return <span className="shortcut-binding">
    {parseBinding(binding).map((chord, index) => <Fragment key={index}>
      {index > 0 && <span className="shortcut-then">then</span>}
      {chordKeys(chord, mac).map((key, keyIndex) => <Fragment key={keyIndex}>
        {keyIndex > 0 && <span className="shortcut-plus" aria-hidden="true">+</span>}
        <kbd>{key}</kbd>
      </Fragment>)}
    </Fragment>)}
  </span>;
}

/** Every shortcut in the registry, grouped and filterable. */
export default function KeyboardShortcutsDialog({ onClose }: { onClose: () => void }) {
  const [filter, setFilter] = useState("");
  const mac = isMacPlatform();
  const shown = SHORTCUTS.filter((def) => shortcutMatches(def, filter, mac));
  return <Modal label="Keyboard shortcuts" title="Keyboard shortcuts" size="lg" onClose={onClose}
    subtitle="Keys only work where they say: workspace keys never fire while you type in a field or a terminal."
    footer={<button className="btn btn-secondary" onClick={onClose}>Close</button>}>
    <FilterField label="Filter shortcuts" value={filter} onChange={setFilter} />
    {shown.length === 0 && <p className="muted">No shortcut matches “{filter}”.</p>}
    {SHORTCUT_GROUPS.map((group) => {
      const rows = shown.filter((def) => def.group === group);
      if (rows.length === 0) return null;
      const headingId = `shortcut-group-${group.replace(/\W+/g, "-").toLowerCase()}`;
      return <section key={group} className="shortcut-group" aria-labelledby={headingId}>
        <h3 id={headingId}>{group}</h3>
        <dl className="shortcut-list">
          {rows.map((def) => <div key={def.id} className="shortcut-row" data-shortcut-id={def.id}>
            <dt className="shortcut-keys">
              {def.keys.map((binding, index) => <Fragment key={binding}>
                {index > 0 && <span className="shortcut-or"> or </span>}
                <Binding binding={binding} mac={mac} />
              </Fragment>)}
            </dt>
            <dd>
              <span className="shortcut-label">{def.label}</span>
              <span className="shortcut-when">{SCOPES[def.scope].when}</span>
            </dd>
          </div>)}
        </dl>
      </section>;
    })}
  </Modal>;
}
