mod dashboard;
mod dialogs;
mod header;
mod list;
mod pane;
mod picker;
mod status;

use ratatui::Frame;

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App) {
    dashboard::draw(frame, app);
}
