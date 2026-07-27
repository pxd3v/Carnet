use std::{
    fs, io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{Hide, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    app::{
        App, AppAction, AppEffect, AppEvent, CatalogSnapshot, EffectExecutor, Focus,
        NavigationAction, OverlayState, Screen,
    },
    catalog::{Catalog, CatalogError},
    cli::Launch,
    editor::{Clipboard, ClipboardError, SystemClipboard},
    git::GitRepo,
    ui,
};

pub trait TerminalLifecycle {
    fn enter(&mut self) -> io::Result<()>;
    fn restore(&mut self) -> io::Result<()>;
}

#[derive(Debug, Default)]
pub struct CrosstermLifecycle;

impl TerminalLifecycle for CrosstermLifecycle {
    fn enter(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide
        )
    }

    fn restore(&mut self) -> io::Result<()> {
        let terminal_result = execute!(
            io::stdout(),
            Show,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let raw_result = disable_raw_mode();
        terminal_result.and(raw_result)
    }
}

#[derive(Debug)]
pub struct RestorationGuard<L: TerminalLifecycle> {
    lifecycle: Option<L>,
}

impl<L: TerminalLifecycle> RestorationGuard<L> {
    pub fn enter(mut lifecycle: L) -> io::Result<Self> {
        if let Err(error) = lifecycle.enter() {
            let _ = lifecycle.restore();
            return Err(error);
        }
        Ok(Self {
            lifecycle: Some(lifecycle),
        })
    }

    pub fn restore(mut self) -> io::Result<()> {
        self.restore_inner()
    }

    fn restore_inner(&mut self) -> io::Result<()> {
        self.lifecycle
            .take()
            .map_or(Ok(()), |mut lifecycle| lifecycle.restore())
    }
}

impl<L: TerminalLifecycle> Drop for RestorationGuard<L> {
    fn drop(&mut self) {
        let _ = self.restore_inner();
    }
}

pub fn map_terminal_event(app: &App, event: Event) -> Option<AppEvent> {
    match event {
        Event::Key(key) => ui::map_key(app, key),
        Event::Paste(text)
            if app.dialog.is_none()
                && matches!(app.overlay, OverlayState::None)
                && matches!(
                    &app.screen,
                    Screen::Workspace(workspace)
                        if workspace.focus == Focus::Editor && workspace.editor.is_some()
                ) =>
        {
            Some(AppEvent::Action(AppAction::Editor(
                crate::editor::EditorCommand::BracketedPaste(text),
            )))
        }
        _ => None,
    }
}

#[derive(Debug, Error)]
pub enum RuntimeDriverError {
    #[error("timed out waiting for background work")]
    BackgroundTimeout,
    #[error("the background effect worker disconnected")]
    BackgroundDisconnected,
}

pub struct Runtime {
    app: App,
    catalog: Catalog,
    executor: EffectExecutor,
    clipboard: ClipboardBoundary,
    result_tx: Sender<AppEvent>,
    result_rx: Receiver<AppEvent>,
    in_flight: usize,
}

impl Runtime {
    pub fn new(catalog: Catalog, launch: Launch) -> Self {
        Self::with_clipboard(catalog, launch, Box::new(SystemClipboard))
    }

    pub fn with_clipboard(catalog: Catalog, launch: Launch, clipboard: Box<dyn Clipboard>) -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        let (app, initial_navigation) = match launch {
            Launch::Home {
                selected_repository,
                pending_note,
            } => {
                let mut app = App::home(
                    catalog.repositories().to_vec(),
                    catalog.default_repository_id(),
                    pending_note,
                );
                if let Some(selected_repository) = selected_repository {
                    app.home.selected = app
                        .home
                        .repositories
                        .iter()
                        .position(|repository| repository.id == selected_repository);
                }
                (app, None)
            }
            Launch::Repository { repository, note } => (
                App::home(
                    catalog.repositories().to_vec(),
                    catalog.default_repository_id(),
                    None,
                ),
                Some(NavigationAction::Repository { repository, note }),
            ),
        };
        let mut runtime = Self {
            app,
            catalog,
            executor: EffectExecutor::default(),
            clipboard: ClipboardBoundary::new(clipboard),
            result_tx,
            result_rx,
            in_flight: 0,
        };
        if let Some(navigation) = initial_navigation {
            runtime.dispatch(AppEvent::Action(AppAction::Navigate(navigation)));
        }
        runtime
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn dispatch(&mut self, event: AppEvent) {
        let effects = self.app.update(event);
        self.dispatch_effects(effects);
    }

    pub fn poll_background(&mut self) -> Result<bool, RuntimeDriverError> {
        let mut handled = false;
        loop {
            match self.result_rx.try_recv() {
                Ok(event) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.dispatch(event);
                    handled = true;
                }
                Err(TryRecvError::Empty) => return Ok(handled),
                Err(TryRecvError::Disconnected) if self.in_flight == 0 => return Ok(handled),
                Err(TryRecvError::Disconnected) => {
                    return Err(RuntimeDriverError::BackgroundDisconnected);
                }
            }
        }
    }

    pub fn wait_for_idle(&mut self, timeout: Duration) -> Result<(), RuntimeDriverError> {
        let deadline = Instant::now() + timeout;
        while self.in_flight > 0 {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(RuntimeDriverError::BackgroundTimeout)?;
            match self.result_rx.recv_timeout(remaining) {
                Ok(event) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.dispatch(event);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(RuntimeDriverError::BackgroundTimeout);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(RuntimeDriverError::BackgroundDisconnected);
                }
            }
        }
        Ok(())
    }

    fn dispatch_effects(&mut self, effects: Vec<AppEffect>) {
        for effect in effects {
            if is_background_effect(&effect) {
                self.spawn_effect(effect);
            } else {
                let event = self.execute_outer_effect(effect);
                self.dispatch(event);
            }
        }
    }

    fn spawn_effect(&mut self, effect: AppEffect) {
        let executor = self.executor.clone();
        let result_tx = self.result_tx.clone();
        self.in_flight += 1;
        thread::spawn(move || {
            let event = executor
                .execute(effect)
                .expect("background effects are handled by EffectExecutor");
            let _ = result_tx.send(event);
        });
    }

    fn execute_outer_effect(&mut self, effect: AppEffect) -> AppEvent {
        match effect {
            AppEffect::ReadClipboard => AppEvent::ClipboardRead(self.clipboard.read_text()),
            AppEffect::WriteClipboard { text } => {
                AppEvent::ClipboardWritten(self.clipboard.write_text(&text))
            }
            AppEffect::SetDefaultRepository { repository_id } => {
                let result = self.set_default(repository_id);
                AppEvent::DefaultRepositoryPersisted {
                    repository_id,
                    result,
                }
            }
            AppEffect::CreateRepository { name, path } => self.create_repository(name, path),
            AppEffect::RegisterRepository { name, path } => self.register_repository(name, path),
            AppEffect::RenameRepository {
                repository_id,
                name,
            } => self.rename_repository(repository_id, name),
            AppEffect::UnregisterRepository { repository_id } => {
                self.unregister_repository(repository_id)
            }
            effect => panic!("background effect reached outer runtime: {effect:?}"),
        }
    }

    fn set_default(&mut self, repository_id: Uuid) -> Result<(), CatalogError> {
        let name = self.repository_name(repository_id)?.to_owned();
        let mut draft = self.catalog.clone();
        draft.set_default(&name)?;
        draft.save()?;
        self.catalog = draft;
        Ok(())
    }

    fn create_repository(&mut self, name: String, path: PathBuf) -> AppEvent {
        if let Err(error) = self.validate_new_name(&name) {
            return catalog_failure(error);
        }
        if let Err(error) = GitRepo::initialize(&path) {
            return catalog_failure(error);
        }
        self.register_validated_repository(name, path)
    }

    fn register_repository(&mut self, name: String, path: PathBuf) -> AppEvent {
        if let Err(error) = self.validate_new_name(&name) {
            return catalog_failure(error);
        }
        let git = match GitRepo::open(&path) {
            Ok(git) => git,
            Err(error) => return catalog_failure(error),
        };
        let canonical = match fs::canonicalize(&path) {
            Ok(path) => path,
            Err(error) => return catalog_failure(error),
        };
        if git.root() != canonical {
            return AppEvent::RepositoryCatalogFailed {
                message: format!(
                    "repository path must be the Git work-tree root: {}",
                    canonical.display()
                ),
            };
        }
        self.register_validated_repository(name, canonical)
    }

    fn register_validated_repository(&mut self, name: String, path: PathBuf) -> AppEvent {
        let mut draft = self.catalog.clone();
        let repository = match draft.register(name, path) {
            Ok(repository) => repository,
            Err(error) => return catalog_failure(error),
        };
        if let Err(error) = draft.save() {
            return catalog_failure(error);
        }
        self.catalog = draft;
        self.catalog_changed(Some(repository.id))
    }

    fn rename_repository(&mut self, repository_id: Uuid, name: String) -> AppEvent {
        let current = match self.repository_name(repository_id) {
            Ok(current) => current.to_owned(),
            Err(error) => return catalog_failure(error),
        };
        let mut draft = self.catalog.clone();
        if let Err(error) = draft.rename_registration(&current, name) {
            return catalog_failure(error);
        }
        if let Err(error) = draft.save() {
            return catalog_failure(error);
        }
        self.catalog = draft;
        self.catalog_changed(Some(repository_id))
    }

    fn unregister_repository(&mut self, repository_id: Uuid) -> AppEvent {
        let name = match self.repository_name(repository_id) {
            Ok(name) => name.to_owned(),
            Err(error) => return catalog_failure(error),
        };
        let mut draft = self.catalog.clone();
        if let Err(error) = draft.unregister(&name) {
            return catalog_failure(error);
        }
        if let Err(error) = draft.save() {
            return catalog_failure(error);
        }
        self.catalog = draft;
        self.catalog_changed(self.catalog.default_repository_id())
    }

    fn catalog_changed(&self, selected_repository: Option<Uuid>) -> AppEvent {
        AppEvent::RepositoryCatalogChanged(CatalogSnapshot {
            repositories: self.catalog.repositories().to_vec(),
            default_repository: self.catalog.default_repository_id(),
            selected_repository,
        })
    }

    fn validate_new_name(&self, name: &str) -> Result<(), CatalogError> {
        if name.trim().is_empty() {
            return Err(CatalogError::EmptyName);
        }
        if self
            .catalog
            .repositories()
            .iter()
            .any(|repository| repository.name == name)
        {
            return Err(CatalogError::DuplicateName {
                name: name.to_owned(),
            });
        }
        Ok(())
    }

    fn repository_name(&self, id: Uuid) -> Result<&str, CatalogError> {
        self.catalog
            .repositories()
            .iter()
            .find(|repository| repository.id == id)
            .map(|repository| repository.name.as_str())
            .ok_or_else(|| CatalogError::RepositoryNotFound {
                name: id.to_string(),
            })
    }
}

fn is_background_effect(effect: &AppEffect) -> bool {
    matches!(
        effect,
        AppEffect::OpenWorkspace { .. }
            | AppEffect::ApplyAndCommit { .. }
            | AppEffect::LoadNote { .. }
            | AppEffect::RetryCommit { .. }
    )
}

fn catalog_failure(error: impl std::fmt::Display) -> AppEvent {
    AppEvent::RepositoryCatalogFailed {
        message: error.to_string(),
    }
}

struct ClipboardBoundary {
    primary: Box<dyn Clipboard>,
    local: String,
}

impl ClipboardBoundary {
    fn new(primary: Box<dyn Clipboard>) -> Self {
        Self {
            primary,
            local: String::new(),
        }
    }
}

impl Clipboard for ClipboardBoundary {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        match self.primary.read_text() {
            Ok(text) => {
                self.local.clone_from(&text);
                Ok(text)
            }
            Err(_) => Ok(self.local.clone()),
        }
    }

    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.local.clear();
        self.local.push_str(text);
        let _ = self.primary.write_text(text);
        Ok(())
    }
}
