//! TUI 模式：全屏终端可视化面板（查询编辑器 + 结果表 + 节点详情）。

mod app;
mod graph;
mod ui;

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

pub fn run(handle: DbHandle, path: &str, limit: usize) -> CliResult {
    let mut terminal = setup_terminal()?;

    let mut app = App::new(handle, path.to_string(), limit);
    app.initial_load();

    let res = run_loop(&mut terminal, &mut app);

    restore_terminal(&mut terminal)?;
    res
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
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
