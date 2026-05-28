//! TUI 应用状态与事件处理。

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use triviumdb::node::NodeId;

use crate::db_handle::{CliNode, CliRows, DbHandle};

/// 当前活跃面板。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Query,
    Results,
}

/// 左下区显示内容：结果表格 / 力导向图。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LeftView {
    Results,
    Graph,
}

/// TUI 全局状态。
pub struct App {
    pub handle: DbHandle,
    pub path: String,
    /// 查询编辑器缓冲（按字符存储，避免多字节越界）
    pub query: Vec<char>,
    pub cursor: usize,
    pub rows: CliRows,
    pub selected: usize,
    pub detail: Option<CliNode>,
    pub table_state: TableState,
    pub status: String,
    pub last_elapsed: Option<Duration>,
    pub focus: Focus,
    pub left_view: LeftView,
    pub show_help: bool,
    pub should_quit: bool,
}

// 注意：TQL 不允许空的 `FIND {}`（会报“文档过滤不能为空”），
// 因此用 Cypher 风格的 `MATCH (n)` 作为“列出全部节点”的默认查询。
const DEFAULT_QUERY: &str = "MATCH (n) RETURN n LIMIT 50";

impl App {
    pub fn new(handle: DbHandle, path: String) -> Self {
        App {
            handle,
            path,
            query: DEFAULT_QUERY.chars().collect(),
            cursor: DEFAULT_QUERY.chars().count(),
            rows: Vec::new(),
            selected: 0,
            detail: None,
            table_state: TableState::default(),
            status: String::from("就绪"),
            last_elapsed: None,
            focus: Focus::Query,
            left_view: LeftView::Results,
            show_help: false,
            should_quit: false,
        }
    }

    pub fn query_string(&self) -> String {
        self.query.iter().collect()
    }

    /// 启动时执行默认查询并把焦点切到结果面板。
    pub fn initial_load(&mut self) {
        self.execute_query();
        self.focus = Focus::Results;
    }

    /// 执行当前查询（自动判定读 / 写）。
    pub fn execute_query(&mut self) {
        let q = self.query_string();
        let q = q.trim().trim_end_matches(';').trim().to_string();
        if q.is_empty() {
            self.status = "（空查询）".into();
            return;
        }

        let start = Instant::now();
        if is_mutation(&q) {
            match self.handle.tql_mut(&q) {
                Ok(s) => {
                    let _ = self.handle.flush();
                    self.status = format!("写入成功 affected={}, created_ids={:?}", s.affected, s.created_ids);
                }
                Err(e) => self.status = format!("错误: {e}"),
            }
        } else {
            match self.handle.tql(&q) {
                Ok(mut rows) => {
                    // 结果按主节点 id 排序，避免 MemTable 槽位序造成的乱序
                    rows.sort_by_key(super::graph::row_primary_id);
                    self.rows = rows;
                    self.selected = 0;
                    self.table_state.select(if self.rows.is_empty() { None } else { Some(0) });
                    self.status = format!("{} 行结果", self.rows.len());
                    self.update_detail();
                }
                Err(e) => {
                    self.status = format!("错误: {e}");
                }
            }
        }
        self.last_elapsed = Some(start.elapsed());
    }

    fn selected_node_id(&self) -> Option<NodeId> {
        let row = self.rows.get(self.selected)?;
        let mut keys: Vec<&String> = row.keys().collect();
        keys.sort();
        let first = keys.first()?;
        Some(row.get(*first)?.id)
    }

    /// 刷新右侧节点详情（重新从 DB 取完整信息，避免投影裁剪导致缺字段）。
    fn update_detail(&mut self) {
        let id = self.selected_node_id();
        self.detail = id.and_then(|id| self.handle.get(id));
    }

    fn select_next(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.rows.len() - 1);
        self.table_state.select(Some(self.selected));
        self.update_detail();
    }

    fn select_prev(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = self.selected.saturating_sub(1);
        self.table_state.select(Some(self.selected));
        self.update_detail();
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // 全局：Ctrl-C 始终退出
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        // 帮助浮层：任意键关闭
        if self.show_help {
            self.show_help = false;
            return;
        }
        match self.focus {
            Focus::Query => self.on_key_query(key),
            Focus::Results => self.on_key_results(key),
        }
    }

    fn on_key_query(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.execute_query(),
            KeyCode::Tab => self.focus = Focus::Results,
            KeyCode::Esc => self.focus = Focus::Results,
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.query.remove(self.cursor - 1);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.query.len() {
                    self.query.remove(self.cursor);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                if self.cursor < self.query.len() {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.query.len(),
            KeyCode::Char(c) => {
                self.query.insert(self.cursor, c);
                self.cursor += 1;
            }
            _ => {}
        }
    }

    fn on_key_results(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('/') | KeyCode::Tab => self.focus = Focus::Query,
            KeyCode::Char('g') => self.toggle_left_view(),
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev(),
            _ => {}
        }
    }

    fn toggle_left_view(&mut self) {
        self.left_view = match self.left_view {
            LeftView::Results => LeftView::Graph,
            LeftView::Graph => LeftView::Results,
        };
    }
}

/// 通过首关键词粗判是否为写操作。
fn is_mutation(query: &str) -> bool {
    let up = query.trim_start().to_ascii_uppercase();
    ["CREATE", "SET", "DELETE", "DETACH", "MERGE", "REMOVE"]
        .iter()
        .any(|kw| up.starts_with(kw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_handle::{DType, DbHandle};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// 创建一个隔离的临时数据库并预置 2 个 person 节点。
    fn temp_app() -> App {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("tdb_tui_test_{}_{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.tdb").to_string_lossy().to_string();
        let mut h = DbHandle::open(&path, 4, DType::F32).unwrap();
        h.insert_f32(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice", "type": "person"}))
            .unwrap();
        h.insert_f32(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "Bob", "type": "person"}))
            .unwrap();
        App::new(h, path)
    }

    #[test]
    fn default_query_populates_rows() {
        let mut app = temp_app();
        app.initial_load();
        assert_eq!(app.rows.len(), 2);
        assert!(app.detail.is_some());
        assert_eq!(app.focus, Focus::Results);
    }

    #[test]
    fn navigation_updates_selection_and_quit() {
        let mut app = temp_app();
        app.initial_load();
        assert_eq!(app.selected, 0);
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.selected, 1);
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.selected, 0);
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_quit);
    }

    #[test]
    fn editing_and_executing_query() {
        let mut app = temp_app();
        app.focus = Focus::Query;
        app.query.clear();
        app.cursor = 0;
        for c in "FIND {type: \"person\"} RETURN *".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.rows.len(), 2);
    }

    #[test]
    fn ctrl_c_quits_from_any_focus() {
        let mut app = temp_app();
        app.focus = Focus::Query;
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn renders_without_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = temp_app();
        app.initial_load();

        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| super::super::ui::render(f, &mut app))
            .unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("TriviumDB"));
        assert!(content.contains("Results"));
        assert!(content.contains("Node Detail"));
    }
}
