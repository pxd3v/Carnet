use std::{io, process::ExitCode, time::Duration};

use carnet::{
    app::AppExitStatus,
    catalog::Catalog,
    cli::{Cli, Invocation, resolve_invocation},
    note_output::write_note_output,
    runtime::{
        CrosstermLifecycle, DEFAULT_QUIT_GRACE, RestorationGuard, Runtime, map_terminal_event,
    },
    ui,
};
use clap::Parser;
use crossterm::event;
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let catalog = match Catalog::load() {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("carnet: {error}");
            return ExitCode::from(2);
        }
    };
    let invocation = match resolve_invocation(cli, &catalog) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("carnet: {error}");
            return ExitCode::from(2);
        }
    };
    match invocation {
        Invocation::Interactive(launch) => {
            let mut runtime = Runtime::new(catalog, launch);
            match run_tui(&mut runtime) {
                Ok(status) => ExitCode::from(status.code()),
                Err(error) => {
                    eprintln!("carnet: terminal runtime failed: {error}");
                    ExitCode::from(1)
                }
            }
        }
        Invocation::NoteOutput(request) => {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            match write_note_output(request, &mut stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("carnet: {error}");
                    ExitCode::from(1)
                }
            }
        }
    }
}

fn run_tui(runtime: &mut Runtime) -> io::Result<AppExitStatus> {
    let guard = RestorationGuard::enter(CrosstermLifecycle::default())?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let result = drive_terminal(&mut terminal, runtime);
    let restore = guard.restore();
    match (result, restore) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(status), Ok(())) => Ok(status),
    }
}

fn drive_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    runtime: &mut Runtime,
) -> io::Result<AppExitStatus> {
    loop {
        runtime.poll_background().map_err(io::Error::other)?;
        terminal.draw(|frame| ui::render(frame, runtime.app()))?;

        if runtime.app().quit.requested {
            return Ok(runtime.finalize_quit(DEFAULT_QUIT_GRACE));
        }

        if event::poll(Duration::from_millis(50))?
            && let Some(app_event) = map_terminal_event(runtime.app(), event::read()?)
        {
            runtime.dispatch(app_event);
        }
    }
}
