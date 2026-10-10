//! Bounded, read-only desktop access to AMF's shared debug history.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::gui_contract::{GuiHandle, GuiResult};

#[derive(Debug, Serialize)]
pub struct DebugLogView {
    /// Oldest first, matching the TUI. Filters apply within this recent window.
    pub entries: Vec<DebugLogEntry>,
    pub limit: usize,
    pub shared_history: bool,
}

#[derive(Debug, Serialize)]
pub struct DebugLogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: &'static str,
    pub context: String,
    pub message: String,
}

pub fn load(gui: &mut GuiHandle) -> GuiResult<DebugLogView> {
    let app = gui.app_for_workflow();
    Ok(DebugLogView {
        entries: app
            .recent_debug_log_entries()?
            .into_iter()
            .map(|entry| DebugLogEntry {
                timestamp: entry.timestamp,
                level: entry.level.display(),
                context: entry.context,
                message: entry.message,
            })
            .collect(),
        limit: app.debug_log.max_entries(),
        shared_history: app.db.is_some(),
    })
}
