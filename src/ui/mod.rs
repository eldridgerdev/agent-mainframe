mod dashboard;
mod header;
mod list;
mod status;
mod dialogs;
mod pane;
mod picker;

use ratatui::Frame;

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App) {
    dashboard::draw(frame, app);
}
