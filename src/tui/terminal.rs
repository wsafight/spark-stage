use std::io::{self, IsTerminal, Stdout};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use thiserror::Error;

use super::TuiOptions;
use super::app::{App, StatusKind, StatusMessage};
use super::backend::UnixBackend;
use super::ui;

const FRAME_INTERVAL: Duration = Duration::from_millis(100);

type CrosstermTerminal = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Error)]
pub enum TuiError {
    #[error("the TUI requires interactive stdin and stdout terminals")]
    NotInteractive,
    #[error("terminal I/O failed: {0}")]
    Io(#[from] io::Error),
}

pub fn run(options: TuiOptions) -> Result<(), TuiError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(TuiError::NotInteractive);
    }

    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(|panic_info| {
        restore_terminal_best_effort();
        eprintln!("SparkStage TUI panicked: {panic_info}");
    }));

    let result = panic::catch_unwind(AssertUnwindSafe(|| run_inner(options)));
    let _temporary_hook = panic::take_hook();
    panic::set_hook(previous_hook);

    match result {
        Ok(result) => result,
        Err(payload) => panic::resume_unwind(payload),
    }
}

fn run_inner(options: TuiOptions) -> Result<(), TuiError> {
    let backend = UnixBackend::new(options.socket, options.project_id);
    let mut app = App::new(backend, options.refresh_interval);
    app.initial_refresh();

    let mut session = TerminalSession::enter()?;
    let mut next_frame = Instant::now();

    while !app.should_quit {
        let now = Instant::now();
        app.tick(now);

        if now >= next_frame {
            session.terminal.draw(|frame| ui::render(frame, &app))?;
            next_frame = now + FRAME_INTERVAL;
        }

        let wait = next_frame.saturating_duration_since(Instant::now());
        if event::poll(wait)?
            && let Event::Key(key) = event::read()?
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                app.should_quit = true;
            } else {
                app.handle_key(key);
            }
        }

        if let Some(path) = app.take_pending_artifact() {
            open_artifact(&mut session, &mut app, &path)?;
            next_frame = Instant::now();
        }
    }

    session.restore()?;
    Ok(())
}

struct TerminalSession {
    terminal: CrosstermTerminal,
    active: bool,
}

impl TerminalSession {
    fn enter() -> Result<Self, TuiError> {
        enable_raw_mode()?;
        let mut output = io::stdout();
        if let Err(source) = execute!(output, EnterAlternateScreen, Hide) {
            restore_terminal_best_effort();
            return Err(TuiError::Io(source));
        }

        let backend = CrosstermBackend::new(output);
        let terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(source) => {
                restore_terminal_best_effort();
                return Err(TuiError::Io(source));
            }
        };
        Ok(Self {
            terminal,
            active: true,
        })
    }

    fn suspend(&mut self) -> Result<(), TuiError> {
        if !self.active {
            return Ok(());
        }
        self.active = false;

        let screen_result = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show);
        let raw_result = disable_raw_mode();
        match (screen_result, raw_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(source), _) | (_, Err(source)) => {
                restore_terminal_best_effort();
                Err(TuiError::Io(source))
            }
        }
    }

    fn resume(&mut self) -> Result<(), TuiError> {
        if self.active {
            return Ok(());
        }
        enable_raw_mode()?;
        if let Err(source) = execute!(self.terminal.backend_mut(), EnterAlternateScreen, Hide) {
            restore_terminal_best_effort();
            return Err(TuiError::Io(source));
        }
        self.active = true;
        self.terminal.clear()?;
        Ok(())
    }

    fn restore(&mut self) -> Result<(), TuiError> {
        if self.active {
            self.suspend()?;
        }
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show);
            let _ = disable_raw_mode();
            self.active = false;
        }
    }
}

fn open_artifact<B: super::backend::TuiBackend>(
    session: &mut TerminalSession,
    app: &mut App<B>,
    path: &Path,
) -> Result<(), TuiError> {
    session.suspend()?;
    println!("Opening {}", path.display());
    let result = launch_artifact(path);
    session.resume()?;

    app.status = match result {
        Ok(launcher) => StatusMessage {
            kind: StatusKind::Success,
            text: format!("Opened {} with {launcher}", path.display()),
        },
        Err(message) => StatusMessage {
            kind: StatusKind::Error,
            text: format!("{message}; artifact remains at {}", path.display()),
        },
    };
    Ok(())
}

fn launch_artifact(path: &Path) -> Result<String, String> {
    let (mut command, launcher) = if let Some(player) = std::env::var_os("SPARKSTAGE_PLAYER") {
        if player.is_empty() {
            return Err("SPARKSTAGE_PLAYER is empty".to_owned());
        }
        let display = player.to_string_lossy().into_owned();
        (Command::new(player), display)
    } else if cfg!(target_os = "macos") {
        (Command::new("open"), "system opener".to_owned())
    } else {
        (Command::new("xdg-open"), "system opener".to_owned())
    };

    let status = command
        .arg(path)
        .status()
        .map_err(|error| format!("cannot start {launcher}: {error}"))?;
    if status.success() {
        Ok(launcher)
    } else {
        Err(format!("{launcher} exited with {status}"))
    }
}

fn restore_terminal_best_effort() {
    let _ = disable_raw_mode();
    let mut output = io::stdout();
    let _ = execute!(output, LeaveAlternateScreen, Show);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_player_name_is_not_shell_parsed() {
        let path = Path::new("/tmp/take with spaces.mp4");
        let mut command = Command::new("player --unsafe-option");
        command.arg(path);
        let debug = format!("{command:?}");

        assert!(debug.contains("player --unsafe-option"));
        assert!(debug.contains("take with spaces.mp4"));
    }
}
