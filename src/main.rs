use std::{io, process::ExitCode, time::Duration};

use carnet::{
    app::AppExitStatus,
    catalog::Catalog,
    cli::{Cli, route},
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
    let launch = match route(cli, &catalog) {
        Ok(launch) => launch,
        Err(error) => {
            eprintln!("carnet: {error}");
            return ExitCode::from(2);
        }
    };
    let mut runtime = Runtime::new(catalog, launch);
    match run_tui(&mut runtime) {
        Ok(status) => ExitCode::from(status.code()),
        Err(error) => {
            eprintln!("carnet: terminal runtime failed: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_tui(runtime: &mut Runtime) -> io::Result<AppExitStatus> {
    let guard = RestorationGuard::enter(CrosstermLifecycle)?;
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
