mod dashboard;
mod dialogs;
mod header;
mod list;
mod pane;
mod picker;
mod status;

use ratatui::Frame;

use crate::app::App;

pub(crate) use dialogs::{latest_prompt_dialog_layout, latest_prompt_selected_text};

pub fn draw(frame: &mut Frame, app: &mut App) {
    dashboard::draw(frame, app);
}
