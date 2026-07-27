mod dialogs;
mod home;
mod keymap;
mod workspace;

use ratatui::Frame;

use crate::app::{App, Screen};

pub use keymap::map_key;
pub use workspace::{WorkspaceGeometry, workspace_geometry};

/// Below this width the editor retains the full workspace and the tree floats over it.
pub const COMFORTABLE_WIDTH: u16 = 96;

pub fn render(frame: &mut Frame<'_>, app: &App) {
    match &app.screen {
        Screen::Home => home::render(frame, app),
        Screen::Workspace(workspace) => workspace::render(frame, app, workspace),
    }
    dialogs::render(frame, app);
}
