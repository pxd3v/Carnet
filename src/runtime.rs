use std::{
    collections::HashMap,
    ffi::OsString,
    fs, io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
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
        App, AppAction, AppEffect, AppEvent, CatalogSnapshot, EffectExecutor, Focus, MutationId,
        NavigationAction, OverlayState, RequestId, RuntimeError, RuntimeOperation, Screen,
    },
    catalog::{Catalog, CatalogError},
    cli::Launch,
    editor::{Clipboard, ClipboardError, SystemClipboard},
    git::{GitError, GitRepo, MutationCommitError},
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerKind {
    OpenWorkspace,
    LoadNote,
    Mutation,
    RetryCommit,
    Catalog,
    ClipboardRead,
    ClipboardWrite,
}

pub trait WorkerHook: Send + Sync {
    fn before_execute(&self, _kind: WorkerKind) {}
}

struct NoopWorkerHook;

impl WorkerHook for NoopWorkerHook {}

#[derive(Debug, Error)]
pub enum RuntimeDriverError {
    #[error("timed out waiting for background work")]
    BackgroundTimeout,
    #[error("the supervised worker completion channel closed")]
    CompletionChannelClosed,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct JobId(u64);

#[derive(Clone, Debug)]
enum WorkerOrigin {
    OpenWorkspace {
        request_id: RequestId,
        repository_id: Uuid,
    },
    LoadNote {
        request_id: RequestId,
        repository_id: Uuid,
    },
    Mutation {
        mutation_id: MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
    },
    RetryCommit {
        mutation_id: MutationId,
        repository_id: Uuid,
        repository_root: PathBuf,
    },
    Catalog,
    ClipboardRead,
    ClipboardWrite,
}

impl WorkerOrigin {
    fn for_effect(effect: &AppEffect) -> Self {
        match effect {
            AppEffect::OpenWorkspace {
                request_id,
                repository,
                ..
            } => Self::OpenWorkspace {
                request_id: *request_id,
                repository_id: repository.id,
            },
            AppEffect::LoadNote {
                request_id,
                repository_id,
                ..
            } => Self::LoadNote {
                request_id: *request_id,
                repository_id: *repository_id,
            },
            AppEffect::ApplyAndCommit {
                mutation_id,
                repository_id,
                repository_root,
                ..
            } => Self::Mutation {
                mutation_id: *mutation_id,
                repository_id: *repository_id,
                repository_root: repository_root.clone(),
            },
            AppEffect::RetryCommit {
                mutation_id,
                repository_id,
                repository_root,
                ..
            } => Self::RetryCommit {
                mutation_id: *mutation_id,
                repository_id: *repository_id,
                repository_root: repository_root.clone(),
            },
            AppEffect::ReadClipboard => Self::ClipboardRead,
            AppEffect::WriteClipboard { .. } => Self::ClipboardWrite,
            AppEffect::SetDefaultRepository { .. }
            | AppEffect::CreateRepository { .. }
            | AppEffect::RegisterRepository { .. }
            | AppEffect::RenameRepository { .. }
            | AppEffect::UnregisterRepository { .. } => Self::Catalog,
        }
    }

    fn kind(&self) -> WorkerKind {
        match self {
            Self::OpenWorkspace { .. } => WorkerKind::OpenWorkspace,
            Self::LoadNote { .. } => WorkerKind::LoadNote,
            Self::Mutation { .. } => WorkerKind::Mutation,
            Self::RetryCommit { .. } => WorkerKind::RetryCommit,
            Self::Catalog => WorkerKind::Catalog,
            Self::ClipboardRead => WorkerKind::ClipboardRead,
            Self::ClipboardWrite => WorkerKind::ClipboardWrite,
        }
    }

    fn panic_event(&self) -> AppEvent {
        match self {
            Self::OpenWorkspace {
                request_id,
                repository_id,
            } => AppEvent::RuntimeFailed {
                request_id: *request_id,
                repository_id: *repository_id,
                operation: RuntimeOperation::OpenWorkspace,
                error: RuntimeError::WorkerPanicked {
                    operation: "workspace open",
                },
            },
            Self::LoadNote {
                request_id,
                repository_id,
            } => AppEvent::RuntimeFailed {
                request_id: *request_id,
                repository_id: *repository_id,
                operation: RuntimeOperation::LoadNote,
                error: RuntimeError::WorkerPanicked {
                    operation: "note load",
                },
            },
            Self::Mutation {
                mutation_id,
                repository_id,
                repository_root,
            } => AppEvent::MutationFailed {
                mutation_id: *mutation_id,
                repository_id: *repository_id,
                repository_root: repository_root.clone(),
                error: MutationCommitError::Runtime {
                    message: "worker panicked".into(),
                },
            },
            Self::RetryCommit {
                mutation_id,
                repository_id,
                repository_root,
            } => AppEvent::CommitRetryFailed {
                mutation_id: *mutation_id,
                repository_id: *repository_id,
                repository_root: repository_root.clone(),
                error: GitError::WorkerPanicked {
                    operation: "commit retry",
                },
            },
            Self::Catalog => AppEvent::RepositoryCatalogFailed {
                message: "background catalog worker panicked".into(),
            },
            Self::ClipboardRead => AppEvent::ClipboardRead(Err(ClipboardError::Unavailable)),
            Self::ClipboardWrite => AppEvent::ClipboardWritten(Err(ClipboardError::Unavailable)),
        }
    }
}

struct WorkerJob {
    id: JobId,
    origin: WorkerOrigin,
    effect: AppEffect,
}

struct WorkerCompletion {
    id: JobId,
    event: AppEvent,
}

pub struct Runtime {
    app: App,
    catalog_tx: Option<Sender<WorkerJob>>,
    clipboard_tx: Option<Sender<WorkerJob>>,
    effect_tx: Option<Sender<WorkerJob>>,
    completion_rx: Receiver<WorkerCompletion>,
    jobs: HashMap<JobId, WorkerOrigin>,
    next_job_id: u64,
    service_handles: Vec<JoinHandle<()>>,
}

impl Runtime {
    pub fn new(catalog: Catalog, launch: Launch) -> Self {
        Self::with_clipboard(catalog, launch, Box::new(SystemClipboard))
    }

    pub fn with_clipboard(catalog: Catalog, launch: Launch, clipboard: Box<dyn Clipboard>) -> Self {
        Self::with_clipboard_and_hook(catalog, launch, clipboard, Arc::new(NoopWorkerHook))
    }

    pub fn with_clipboard_and_hook(
        catalog: Catalog,
        launch: Launch,
        clipboard: Box<dyn Clipboard>,
        hook: Arc<dyn WorkerHook>,
    ) -> Self {
        let (app, initial_navigation) = initial_app(&catalog, launch);
        let executor = EffectExecutor::default();
        let catalog = Arc::new(Mutex::new(catalog));
        let (completion_tx, completion_rx) = mpsc::channel();
        let (catalog_tx, catalog_rx) = mpsc::channel();
        let (clipboard_tx, clipboard_rx) = mpsc::channel();
        let (effect_tx, effect_rx) = mpsc::channel();

        let catalog_handle = spawn_catalog_service(
            catalog,
            catalog_rx,
            completion_tx.clone(),
            executor.clone(),
            Arc::clone(&hook),
        );
        let clipboard_handle = spawn_clipboard_service(
            ClipboardBoundary::new(clipboard),
            clipboard_rx,
            completion_tx.clone(),
            Arc::clone(&hook),
        );
        let effect_handle =
            spawn_effect_service(executor, effect_rx, completion_tx, Arc::clone(&hook));

        let mut runtime = Self {
            app,
            catalog_tx: Some(catalog_tx),
            clipboard_tx: Some(clipboard_tx),
            effect_tx: Some(effect_tx),
            completion_rx,
            jobs: HashMap::new(),
            next_job_id: 1,
            service_handles: vec![catalog_handle, clipboard_handle, effect_handle],
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
        for effect in effects {
            self.queue_effect(effect);
        }
    }

    pub fn poll_background(&mut self) -> Result<bool, RuntimeDriverError> {
        let mut handled = false;
        loop {
            match self.completion_rx.try_recv() {
                Ok(completion) => {
                    self.finish_job(completion);
                    handled = true;
                }
                Err(TryRecvError::Empty) => return Ok(handled),
                Err(TryRecvError::Disconnected) => {
                    return Err(RuntimeDriverError::CompletionChannelClosed);
                }
            }
        }
    }

    pub fn wait_for_idle(&mut self, timeout: Duration) -> Result<(), RuntimeDriverError> {
        let deadline = Instant::now() + timeout;
        while !self.jobs.is_empty() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(RuntimeDriverError::BackgroundTimeout)?;
            match self.completion_rx.recv_timeout(remaining) {
                Ok(completion) => self.finish_job(completion),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(RuntimeDriverError::BackgroundTimeout);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(RuntimeDriverError::CompletionChannelClosed);
                }
            }
        }
        Ok(())
    }

    fn queue_effect(&mut self, effect: AppEffect) {
        let origin = WorkerOrigin::for_effect(&effect);
        let id = JobId(self.next_job_id);
        self.next_job_id = self
            .next_job_id
            .checked_add(1)
            .expect("runtime job ID overflow");
        self.jobs.insert(id, origin.clone());
        let job = WorkerJob { id, origin, effect };
        let send_result = match job.origin.kind() {
            WorkerKind::Catalog => self.catalog_tx.as_ref().expect("catalog service").send(job),
            WorkerKind::ClipboardRead | WorkerKind::ClipboardWrite => self
                .clipboard_tx
                .as_ref()
                .expect("clipboard service")
                .send(job),
            WorkerKind::OpenWorkspace
            | WorkerKind::LoadNote
            | WorkerKind::Mutation
            | WorkerKind::RetryCommit => self.effect_tx.as_ref().expect("effect service").send(job),
        };
        if let Err(error) = send_result {
            let job = error.0;
            self.jobs.remove(&job.id);
            self.dispatch(job.origin.panic_event());
        }
    }

    fn finish_job(&mut self, completion: WorkerCompletion) {
        if self.jobs.remove(&completion.id).is_some() {
            self.dispatch(completion.event);
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.catalog_tx.take();
        self.clipboard_tx.take();
        self.effect_tx.take();
        for handle in self.service_handles.drain(..) {
            let _ = handle.join();
        }
    }
}

fn initial_app(catalog: &Catalog, launch: Launch) -> (App, Option<NavigationAction>) {
    match launch {
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
    }
}

fn spawn_catalog_service(
    catalog: Arc<Mutex<Catalog>>,
    receiver: Receiver<WorkerJob>,
    completion: Sender<WorkerCompletion>,
    executor: EffectExecutor,
    hook: Arc<dyn WorkerHook>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("carnet-catalog".into())
        .spawn(move || {
            for job in receiver {
                let completion_message = supervise(job, &hook, |effect| {
                    execute_catalog_effect(&catalog, &executor, effect)
                });
                if completion.send(completion_message).is_err() {
                    break;
                }
            }
        })
        .expect("could not start catalog worker")
}

fn spawn_clipboard_service(
    mut clipboard: ClipboardBoundary,
    receiver: Receiver<WorkerJob>,
    completion: Sender<WorkerCompletion>,
    hook: Arc<dyn WorkerHook>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("carnet-clipboard".into())
        .spawn(move || {
            for job in receiver {
                let completion_message = supervise(job, &hook, |effect| match effect {
                    AppEffect::ReadClipboard => AppEvent::ClipboardRead(clipboard.read_text()),
                    AppEffect::WriteClipboard { text } => {
                        AppEvent::ClipboardWritten(clipboard.write_text(&text))
                    }
                    effect => WorkerOrigin::for_effect(&effect).panic_event(),
                });
                if completion.send(completion_message).is_err() {
                    break;
                }
            }
        })
        .expect("could not start clipboard worker")
}

fn spawn_effect_service(
    executor: EffectExecutor,
    receiver: Receiver<WorkerJob>,
    completion: Sender<WorkerCompletion>,
    hook: Arc<dyn WorkerHook>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("carnet-effect-dispatch".into())
        .spawn(move || {
            let mut handles = Vec::new();
            for job in receiver {
                join_finished(&mut handles);
                let completion = completion.clone();
                let worker_completion = completion.clone();
                let executor = executor.clone();
                let hook = Arc::clone(&hook);
                let failed_id = job.id;
                let failed_event = job.origin.panic_event();
                match thread::Builder::new()
                    .name(format!("carnet-{:?}-{}", job.origin.kind(), job.id.0))
                    .spawn(move || {
                        let completion_message = supervise(job, &hook, |effect| {
                            executor
                                .execute(effect)
                                .unwrap_or_else(|error| error.into_effect().into_failure_event())
                        });
                        let _ = worker_completion.send(completion_message);
                    }) {
                    Ok(handle) => handles.push(handle),
                    Err(_) => {
                        let _ = completion.send(WorkerCompletion {
                            id: failed_id,
                            event: failed_event,
                        });
                    }
                }
            }
            for handle in handles {
                let _ = handle.join();
            }
        })
        .expect("could not start effect dispatcher")
}

fn join_finished(handles: &mut Vec<JoinHandle<()>>) {
    let mut index = 0;
    while index < handles.len() {
        if handles[index].is_finished() {
            let handle = handles.swap_remove(index);
            let _ = handle.join();
        } else {
            index += 1;
        }
    }
}

fn supervise(
    job: WorkerJob,
    hook: &Arc<dyn WorkerHook>,
    operation: impl FnOnce(AppEffect) -> AppEvent,
) -> WorkerCompletion {
    let id = job.id;
    let origin = job.origin;
    let event = catch_unwind(AssertUnwindSafe(|| {
        hook.before_execute(origin.kind());
        operation(job.effect)
    }))
    .unwrap_or_else(|_| origin.panic_event());
    WorkerCompletion { id, event }
}

trait UnsupportedEffectFailure {
    fn into_failure_event(self) -> AppEvent;
}

impl UnsupportedEffectFailure for AppEffect {
    fn into_failure_event(self) -> AppEvent {
        WorkerOrigin::for_effect(&self).panic_event()
    }
}

fn execute_catalog_effect(
    catalog: &Arc<Mutex<Catalog>>,
    executor: &EffectExecutor,
    effect: AppEffect,
) -> AppEvent {
    let mut catalog = catalog.lock().unwrap_or_else(|error| error.into_inner());
    let result = match effect {
        AppEffect::SetDefaultRepository { repository_id } => {
            set_default(&mut catalog, repository_id).map(|()| repository_id)
        }
        AppEffect::CreateRepository { name, path } => {
            create_repository(&mut catalog, executor, name, path)
        }
        AppEffect::RegisterRepository { name, path } => {
            register_repository(&mut catalog, executor, name, path)
        }
        AppEffect::RenameRepository {
            repository_id,
            name,
        } => rename_repository(&mut catalog, repository_id, name).map(|()| repository_id),
        AppEffect::UnregisterRepository { repository_id } => {
            unregister_repository(&mut catalog, repository_id)
                .map(|()| catalog.default_repository_id().unwrap_or(repository_id))
        }
        _ => return WorkerOrigin::Catalog.panic_event(),
    };
    match result {
        Ok(selected_repository) => AppEvent::RepositoryCatalogChanged(CatalogSnapshot {
            repositories: catalog.repositories().to_vec(),
            default_repository: catalog.default_repository_id(),
            selected_repository: catalog
                .repositories()
                .iter()
                .any(|repository| repository.id == selected_repository)
                .then_some(selected_repository)
                .or(catalog.default_repository_id()),
        }),
        Err(message) => AppEvent::RepositoryCatalogFailed { message },
    }
}

fn set_default(catalog: &mut Catalog, repository_id: Uuid) -> Result<(), String> {
    let name = repository_name(catalog, repository_id)?.to_owned();
    let mut draft = catalog.clone();
    draft
        .set_default(&name)
        .map_err(|error| error.to_string())?;
    draft.save().map_err(|error| error.to_string())?;
    *catalog = draft;
    Ok(())
}

fn create_repository(
    catalog: &mut Catalog,
    executor: &EffectExecutor,
    name: String,
    path: PathBuf,
) -> Result<Uuid, String> {
    validate_new_name(catalog, &name).map_err(|error| error.to_string())?;
    let target = with_normalized_root(executor, &path, |target| {
        GitRepo::initialize(target).map_err(|error| error.to_string())?;
        Ok(target.to_path_buf())
    })?;
    register_validated_repository(catalog, name, target)
}

fn register_repository(
    catalog: &mut Catalog,
    executor: &EffectExecutor,
    name: String,
    path: PathBuf,
) -> Result<Uuid, String> {
    validate_new_name(catalog, &name).map_err(|error| error.to_string())?;
    let target = with_normalized_root(executor, &path, |target| {
        let git = GitRepo::open(target).map_err(|error| error.to_string())?;
        if git.root() != target {
            return Err(format!(
                "repository path must be the Git work-tree root: {}",
                target.display()
            ));
        }
        Ok(target.to_path_buf())
    })?;
    register_validated_repository(catalog, name, target)
}

fn register_validated_repository(
    catalog: &mut Catalog,
    name: String,
    path: PathBuf,
) -> Result<Uuid, String> {
    let mut draft = catalog.clone();
    let repository = draft
        .register(name, path)
        .map_err(|error| error.to_string())?;
    draft.save().map_err(|error| error.to_string())?;
    *catalog = draft;
    Ok(repository.id)
}

fn rename_repository(
    catalog: &mut Catalog,
    repository_id: Uuid,
    name: String,
) -> Result<(), String> {
    let current = repository_name(catalog, repository_id)?.to_owned();
    let mut draft = catalog.clone();
    draft
        .rename_registration(&current, name)
        .map_err(|error| error.to_string())?;
    draft.save().map_err(|error| error.to_string())?;
    *catalog = draft;
    Ok(())
}

fn unregister_repository(catalog: &mut Catalog, repository_id: Uuid) -> Result<(), String> {
    let name = repository_name(catalog, repository_id)?.to_owned();
    let mut draft = catalog.clone();
    draft.unregister(&name).map_err(|error| error.to_string())?;
    draft.save().map_err(|error| error.to_string())?;
    *catalog = draft;
    Ok(())
}

fn validate_new_name(catalog: &Catalog, name: &str) -> Result<(), CatalogError> {
    if name.trim().is_empty() {
        return Err(CatalogError::EmptyName);
    }
    if catalog
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

fn repository_name(catalog: &Catalog, id: Uuid) -> Result<&str, String> {
    catalog
        .repositories()
        .iter()
        .find(|repository| repository.id == id)
        .map(|repository| repository.name.as_str())
        .ok_or_else(|| format!("repository ID {id} is not registered"))
}

fn with_normalized_root<T>(
    executor: &EffectExecutor,
    path: &Path,
    operation: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    let lexical = lexical_absolute(path).map_err(|error| error.to_string())?;
    executor.run_for_root(&lexical, || {
        let target = normalize_create_target(&lexical).map_err(|error| error.to_string())?;
        if target == lexical {
            operation(&target)
        } else {
            executor.run_for_root(&target, || operation(&target))
        }
    })
}

fn lexical_absolute(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "repository path traverses above its root",
                    ));
                }
            }
        }
    }
    Ok(normalized)
}

fn normalize_create_target(lexical: &Path) -> io::Result<PathBuf> {
    let mut ancestor = lexical.to_path_buf();
    let mut missing = Vec::<OsString>::new();
    while !ancestor.exists() {
        let component = ancestor.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "repository path has no existing ancestor",
            )
        })?;
        missing.push(component.to_os_string());
        ancestor.pop();
    }
    let mut target = fs::canonicalize(ancestor)?;
    for component in missing.into_iter().rev() {
        target.push(component);
    }
    Ok(target)
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
