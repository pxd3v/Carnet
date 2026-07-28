use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    fs, io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{Hide, Show},
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        supports_keyboard_enhancement,
    },
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    app::{
        App, AppAction, AppEffect, AppEvent, AppExitStatus, CatalogSnapshot, ClipboardRequestId,
        EditorOrigin, EffectExecutor, Focus, MutationId, NavigationAction, OverlayState, PushId,
        RequestId, RuntimeError, RuntimeOperation, Screen,
    },
    catalog::{Catalog, CatalogError},
    cli::Launch,
    editor::{Clipboard, ClipboardError, SystemClipboard},
    git::{GitCancellation, GitError, GitRepo, MutationCommitError},
    ui,
};

pub trait TerminalLifecycle {
    fn enter(&mut self) -> io::Result<()>;
    fn restore(&mut self) -> io::Result<()>;
}

#[derive(Debug, Default)]
pub struct CrosstermLifecycle {
    keyboard: KeyboardEnhancementState,
}

#[derive(Debug, Default)]
struct KeyboardEnhancementState {
    pushed: bool,
}

impl KeyboardEnhancementState {
    fn requested_flags(probe: io::Result<bool>) -> Option<KeyboardEnhancementFlags> {
        probe.ok().filter(|supported| *supported).map(|_| {
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        })
    }

    fn mark_pushed(&mut self) {
        self.pushed = true;
    }

    fn take_pop(&mut self) -> bool {
        std::mem::take(&mut self.pushed)
    }
}

fn enable_keyboard_enhancement(
    state: &mut KeyboardEnhancementState,
    probe: io::Result<bool>,
    push: impl FnOnce(KeyboardEnhancementFlags) -> io::Result<()>,
) -> io::Result<()> {
    let Some(flags) = KeyboardEnhancementState::requested_flags(probe) else {
        return Ok(());
    };
    push(flags)?;
    state.mark_pushed();
    Ok(())
}

fn restore_terminal_modes(
    state: &mut KeyboardEnhancementState,
    pop_keyboard: impl FnOnce() -> io::Result<()>,
    restore_screen: impl FnOnce() -> io::Result<()>,
    restore_raw: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let keyboard_result = if state.take_pop() {
        pop_keyboard()
    } else {
        Ok(())
    };
    let screen_result = restore_screen();
    let raw_result = restore_raw();
    keyboard_result.and(screen_result).and(raw_result)
}

impl TerminalLifecycle for CrosstermLifecycle {
    fn enter(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        enable_keyboard_enhancement(
            &mut self.keyboard,
            supports_keyboard_enhancement(),
            |flags| execute!(io::stdout(), PushKeyboardEnhancementFlags(flags)),
        )?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide
        )
    }

    fn restore(&mut self) -> io::Result<()> {
        restore_terminal_modes(
            &mut self.keyboard,
            || execute!(io::stdout(), PopKeyboardEnhancementFlags),
            || {
                execute!(
                    io::stdout(),
                    Show,
                    DisableBracketedPaste,
                    LeaveAlternateScreen
                )
            },
            disable_raw_mode,
        )
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
    Push,
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

pub const DEFAULT_QUIT_GRACE: Duration = Duration::from_secs(2);
pub const EFFECT_WORKER_COUNT: usize = 2;
const GIT_CANCELLATION_REAP_GRACE: Duration = Duration::from_millis(500);

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
    Push {
        push_id: PushId,
        repository_id: Uuid,
        repository_root: PathBuf,
    },
    Catalog,
    ClipboardRead {
        request_id: ClipboardRequestId,
        origin: EditorOrigin,
    },
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
            AppEffect::Push {
                push_id,
                repository_id,
                repository_root,
                ..
            } => Self::Push {
                push_id: *push_id,
                repository_id: *repository_id,
                repository_root: repository_root.clone(),
            },
            AppEffect::ReadClipboard { request_id, origin } => Self::ClipboardRead {
                request_id: *request_id,
                origin: origin.clone(),
            },
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
            Self::Push { .. } => WorkerKind::Push,
            Self::Catalog => WorkerKind::Catalog,
            Self::ClipboardRead { .. } => WorkerKind::ClipboardRead,
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
            Self::Push {
                push_id,
                repository_id,
                repository_root,
            } => AppEvent::PushFailed {
                push_id: *push_id,
                repository_id: *repository_id,
                repository_root: repository_root.clone(),
                error: GitError::WorkerPanicked { operation: "push" },
            },
            Self::Catalog => AppEvent::RepositoryCatalogFailed {
                message: "background catalog worker panicked".into(),
            },
            Self::ClipboardRead { request_id, origin } => AppEvent::ClipboardRead {
                request_id: *request_id,
                origin: origin.clone(),
                result: Err(ClipboardError::Unavailable),
            },
            Self::ClipboardWrite => AppEvent::ClipboardWritten(Err(ClipboardError::Unavailable)),
        }
    }
}

fn git_cancellation(effect: &AppEffect) -> Option<GitCancellation> {
    match effect {
        AppEffect::ApplyAndCommit { git, .. }
        | AppEffect::RetryCommit { git, .. }
        | AppEffect::Push { git, .. } => Some(git.cancellation()),
        _ => None,
    }
}

struct WorkerJob {
    id: JobId,
    origin: WorkerOrigin,
    effect: AppEffect,
}

impl WorkerJob {
    fn repository_root(&self) -> Option<&Path> {
        match &self.effect {
            AppEffect::OpenWorkspace { repository, .. } => Some(&repository.path),
            AppEffect::LoadNote { workspace, .. } | AppEffect::ApplyAndCommit { workspace, .. } => {
                Some(workspace.root())
            }
            AppEffect::RetryCommit {
                repository_root, ..
            }
            | AppEffect::Push {
                repository_root, ..
            } => Some(repository_root),
            _ => None,
        }
    }

    fn is_coalescible_read(&self) -> bool {
        matches!(
            self.effect,
            AppEffect::OpenWorkspace { .. } | AppEffect::LoadNote { .. }
        )
    }
}

struct WorkerCompletion {
    id: JobId,
    event: AppEvent,
    panicked: bool,
}

#[derive(Default)]
struct EffectQueueState {
    pending: VecDeque<WorkerJob>,
    active_roots: HashSet<PathBuf>,
    closed: bool,
}

#[derive(Default)]
struct EffectQueue {
    state: Mutex<EffectQueueState>,
    ready: Condvar,
}

impl EffectQueue {
    fn enqueue(&self, job: WorkerJob) -> Vec<JobId> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut superseded = Vec::new();
        if job.is_coalescible_read() {
            let root = job
                .repository_root()
                .expect("filesystem reads have a repository root")
                .to_path_buf();
            state.pending.retain(|pending| {
                let remove = pending.is_coalescible_read()
                    && pending.repository_root().is_some_and(|other| other == root);
                if remove {
                    superseded.push(pending.id);
                }
                !remove
            });
        }
        state.pending.push_back(job);
        self.ready.notify_all();
        superseded
    }

    fn take(&self) -> Option<(WorkerJob, PathBuf)> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(index) = state.pending.iter().position(|job| {
                job.repository_root()
                    .is_some_and(|root| !state.active_roots.contains(root))
            }) {
                let job = state
                    .pending
                    .remove(index)
                    .expect("position came from queue");
                let root = job
                    .repository_root()
                    .expect("effect workers receive repository jobs")
                    .to_path_buf();
                state.active_roots.insert(root.clone());
                return Some((job, root));
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn finish(&self, root: &Path) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.active_roots.remove(root);
        self.ready.notify_all();
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.closed = true;
        self.ready.notify_all();
    }
}

pub struct Runtime {
    app: App,
    catalog_tx: Option<Sender<WorkerJob>>,
    clipboard_tx: Option<Sender<WorkerJob>>,
    effect_queue: Arc<EffectQueue>,
    completion_rx: Receiver<WorkerCompletion>,
    jobs: HashMap<JobId, WorkerOrigin>,
    job_cancellations: HashMap<JobId, GitCancellation>,
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
        let effect_queue = Arc::new(EffectQueue::default());

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
        let effect_handles = spawn_effect_workers(
            executor,
            Arc::clone(&effect_queue),
            completion_tx,
            Arc::clone(&hook),
        );
        let mut service_handles = vec![catalog_handle, clipboard_handle];
        service_handles.extend(effect_handles);

        let mut runtime = Self {
            app,
            catalog_tx: Some(catalog_tx),
            clipboard_tx: Some(clipboard_tx),
            effect_queue,
            completion_rx,
            jobs: HashMap::new(),
            job_cancellations: HashMap::new(),
            next_job_id: 1,
            service_handles,
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

    pub fn finalize_quit(&mut self, grace: Duration) -> AppExitStatus {
        if let Some(status) = self.app.quit.final_status {
            return status;
        }

        let deadline = Instant::now() + grace;
        let mut shutdown_deadline = deadline;
        let mut timed_out = false;
        while !self.jobs.is_empty() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                self.record_finalization_timeout();
                timed_out = true;
                break;
            };
            match self.completion_rx.recv_timeout(remaining) {
                Ok(completion) => self.finish_job(completion),
                Err(RecvTimeoutError::Timeout) => {
                    self.record_finalization_timeout();
                    timed_out = true;
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.dispatch(AppEvent::RuntimeFinalizationFailed {
                        message: "background completion channel closed during quit finalization"
                            .into(),
                    });
                    break;
                }
            }
        }

        if timed_out && self.cancel_active_git_jobs() {
            shutdown_deadline = Instant::now() + GIT_CANCELLATION_REAP_GRACE;
            while !self.jobs.is_empty() {
                let Some(remaining) = shutdown_deadline.checked_duration_since(Instant::now())
                else {
                    break;
                };
                match self.completion_rx.recv_timeout(remaining) {
                    Ok(completion) => self.finish_job(completion),
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
                }
            }
        }

        self.dispatch(AppEvent::QuitFinalized);
        self.close_service_inputs();
        self.join_services_until(shutdown_deadline);
        self.app.quit.final_status.unwrap_or(AppExitStatus::Failure)
    }

    fn queue_effect(&mut self, effect: AppEffect) {
        let origin = WorkerOrigin::for_effect(&effect);
        let cancellation = git_cancellation(&effect);
        let id = JobId(self.next_job_id);
        self.next_job_id = self
            .next_job_id
            .checked_add(1)
            .expect("runtime job ID overflow");
        self.jobs.insert(id, origin.clone());
        if let Some(cancellation) = cancellation {
            self.job_cancellations.insert(id, cancellation);
        }
        let job = WorkerJob { id, origin, effect };
        let send_result = match job.origin.kind() {
            WorkerKind::Catalog => {
                Some(self.catalog_tx.as_ref().expect("catalog service").send(job))
            }
            WorkerKind::ClipboardRead | WorkerKind::ClipboardWrite => self
                .clipboard_tx
                .as_ref()
                .expect("clipboard service")
                .send(job)
                .into(),
            WorkerKind::OpenWorkspace
            | WorkerKind::LoadNote
            | WorkerKind::Mutation
            | WorkerKind::RetryCommit
            | WorkerKind::Push => {
                for superseded in self.effect_queue.enqueue(job) {
                    self.jobs.remove(&superseded);
                    self.job_cancellations.remove(&superseded);
                }
                None
            }
        };
        if let Some(Err(error)) = send_result {
            let job = error.0;
            self.jobs.remove(&job.id);
            self.job_cancellations.remove(&job.id);
            self.dispatch(job.origin.panic_event());
        }
    }

    fn finish_job(&mut self, completion: WorkerCompletion) {
        if self.jobs.remove(&completion.id).is_some() {
            self.job_cancellations.remove(&completion.id);
            if completion.panicked && self.app.quit.requested {
                self.dispatch(AppEvent::RuntimeFinalizationFailed {
                    message: "background worker panicked during quit finalization".into(),
                });
            }
            self.dispatch(completion.event);
        }
    }

    fn record_finalization_timeout(&mut self) {
        self.dispatch(AppEvent::RuntimeFinalizationFailed {
            message: format!("timed out finalizing {} background job(s)", self.jobs.len()),
        });
    }

    fn cancel_active_git_jobs(&self) -> bool {
        for cancellation in self.job_cancellations.values() {
            cancellation.cancel();
        }
        !self.job_cancellations.is_empty()
    }

    fn close_service_inputs(&mut self) {
        self.catalog_tx.take();
        self.clipboard_tx.take();
        self.effect_queue.close();
    }

    fn join_services_until(&mut self, deadline: Instant) {
        while !self.service_handles.is_empty() {
            join_finished(&mut self.service_handles);
            if self.service_handles.is_empty() {
                return;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            thread::sleep(remaining.min(Duration::from_millis(1)));
        }
        self.service_handles.clear();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.close_service_inputs();
        join_finished(&mut self.service_handles);
        self.service_handles.clear();
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
                    AppEffect::ReadClipboard { request_id, origin } => AppEvent::ClipboardRead {
                        request_id,
                        origin,
                        result: clipboard.read_text(),
                    },
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

fn spawn_effect_workers(
    executor: EffectExecutor,
    queue: Arc<EffectQueue>,
    completion: Sender<WorkerCompletion>,
    hook: Arc<dyn WorkerHook>,
) -> Vec<JoinHandle<()>> {
    (0..EFFECT_WORKER_COUNT)
        .map(|index| {
            let executor = executor.clone();
            let queue = Arc::clone(&queue);
            let completion = completion.clone();
            let hook = Arc::clone(&hook);
            thread::Builder::new()
                .name(format!("carnet-effect-{index}"))
                .spawn(move || {
                    while let Some((job, root)) = queue.take() {
                        let completion_message = supervise(job, &hook, |effect| {
                            executor
                                .execute(effect)
                                .unwrap_or_else(|error| error.into_effect().into_failure_event())
                        });
                        queue.finish(&root);
                        if completion.send(completion_message).is_err() {
                            break;
                        }
                    }
                })
                .expect("could not start effect worker")
        })
        .collect()
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
    let result = catch_unwind(AssertUnwindSafe(|| {
        hook.before_execute(origin.kind());
        operation(job.effect)
    }));
    let (event, panicked) = match result {
        Ok(event) => (event, false),
        Err(_) => (origin.panic_event(), true),
    };
    WorkerCompletion {
        id,
        event,
        panicked,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyboardEnhancementFlags;

    #[test]
    fn keyboard_enhancement_falls_back_on_false_or_probe_error() {
        assert!(KeyboardEnhancementState::requested_flags(Ok(false)).is_none());
        assert!(
            KeyboardEnhancementState::requested_flags(Err(io::Error::other("probe"))).is_none()
        );
    }

    #[test]
    fn keyboard_enhancement_uses_all_flags_and_pops_once() {
        let flags = KeyboardEnhancementState::requested_flags(Ok(true)).unwrap();
        assert!(flags.contains(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES));

        let mut state = KeyboardEnhancementState::default();
        state.mark_pushed();
        assert!(state.take_pop());
        assert!(!state.take_pop());
    }

    #[test]
    fn keyboard_enablement_owns_only_a_successful_push() {
        let mut state = KeyboardEnhancementState::default();
        let mut pushed = None;
        enable_keyboard_enhancement(&mut state, Ok(true), |flags| {
            pushed = Some(flags);
            Ok(())
        })
        .unwrap();
        assert!(pushed.is_some());
        assert!(state.take_pop());

        for probe in [Ok(false), Err(io::Error::other("probe"))] {
            let mut state = KeyboardEnhancementState::default();
            enable_keyboard_enhancement(&mut state, probe, |_| {
                panic!("fallback must not push keyboard flags")
            })
            .unwrap();
            assert!(!state.take_pop());
        }

        let mut state = KeyboardEnhancementState::default();
        let error =
            enable_keyboard_enhancement(&mut state, Ok(true), |_| Err(io::Error::other("push")))
                .unwrap_err();
        assert_eq!(error.to_string(), "push");
        assert!(!state.take_pop());
    }

    #[test]
    fn keyboard_pop_failure_does_not_skip_remaining_terminal_cleanup() {
        use std::cell::RefCell;

        let mut state = KeyboardEnhancementState::default();
        state.mark_pushed();
        let calls = RefCell::new(Vec::new());
        let error = restore_terminal_modes(
            &mut state,
            || {
                calls.borrow_mut().push("pop");
                Err(io::Error::other("pop"))
            },
            || {
                calls.borrow_mut().push("screen");
                Ok(())
            },
            || {
                calls.borrow_mut().push("raw");
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "pop");
        assert_eq!(*calls.borrow(), ["pop", "screen", "raw"]);
        restore_terminal_modes(
            &mut state,
            || panic!("keyboard flags must pop only once"),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    }
}
