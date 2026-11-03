import { openScreenshotBrowser } from "./screenshotsApi";
import { Menu } from "./ui";

/** The GUI's guide (install, run, and every panel), opened in the browser. */
export const GUI_DOCS_URL = "https://github.com/eldridgerdev/agent-mainframe/blob/main/gui/README.md";

/** The sidebar's Help entry: keyboard shortcuts, the docs, and the first-run
 *  steps for anyone who skipped past them. */
export default function HelpMenu({ onShortcuts, onFirstRun, onError }: {
  onShortcuts: () => void;
  onFirstRun: () => void;
  /** A docs link that could not open (no browser, say). */
  onError: (cause: unknown) => void;
}) {
  return <Menu
    label="Help"
    text="Help"
    icon="help"
    className="nav-item nav-help"
    title="Keyboard shortcuts, documentation and getting started"
    items={[
      { label: "Keyboard shortcuts", icon: "keyboard", shortcut: "help.shortcuts", onSelect: onShortcuts },
      { label: "Documentation", icon: "file", hint: "Opens in your browser", onSelect: () => {
        void openScreenshotBrowser(GUI_DOCS_URL).catch(onError);
      } },
      { label: "Show first-run steps again", icon: "flag", onSelect: onFirstRun },
    ]}
  />;
}
