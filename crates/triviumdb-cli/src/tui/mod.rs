//! TUI 模式：全屏终端可视化面板（查询编辑器 + 结果表 + 节点详情）。

mod app;
mod graph;
mod marker;
mod ui;

pub use marker::GraphMarker;

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};

use crate::CliResult;
use crate::db_handle::DbHandle;
use app::App;

pub fn run(handle: DbHandle, path: &str, limit: usize, marker: GraphMarker) -> CliResult {
    let mut session = setup_terminal()?;

    let mut app = App::new(handle, path.to_string(), limit, marker.resolve());
    app.initial_load();

    let res = run_loop(&mut session.terminal, &mut app);

    session.restore()?;
    res
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        let raw = disable_raw_mode();
        let screen = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let cursor = self.terminal.show_cursor();
        self.restored = true;
        raw?;
        screen?;
        cursor?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn setup_terminal() -> io::Result<TerminalSession> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e);
    }
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(TerminalSession {
            terminal,
            restored: false,
        }),
        Err(e) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            Err(e)
        }
    }
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> CliResult {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        // 仅处理按下事件（Windows 下还会上报 Release/Repeat）
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key);
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
