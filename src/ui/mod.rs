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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutStyle {
    MacOs,
    Portable,
}

impl ShortcutStyle {
    const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Portable
        }
    }
}

pub(super) fn selection_viewport(
    len: usize,
    selected: Option<usize>,
    capacity: usize,
) -> std::ops::Range<usize> {
    let visible = len.min(capacity);
    if visible == 0 {
        return 0..0;
    }
    let selected = selected.unwrap_or(0).min(len.saturating_sub(1));
    let start = selected
        .saturating_sub(visible / 2)
        .min(len.saturating_sub(visible));
    start..start + visible
}

pub fn render(frame: &mut Frame<'_>, app: &App) {
    render_with_shortcut_style(frame, app, ShortcutStyle::current());
}

#[doc(hidden)]
pub fn render_with_shortcut_style(frame: &mut Frame<'_>, app: &App, style: ShortcutStyle) {
    match &app.screen {
        Screen::Home => home::render(frame, app),
        Screen::Workspace(workspace) => workspace::render(frame, app, workspace, style),
    }
    dialogs::render(frame, app);
}
