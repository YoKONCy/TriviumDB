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

/// 图视图的交互状态：k-hop 扩展节点集 + 视口缩放/平移。
pub struct GraphState {
    /// 结果集之外、被 expand 加入的节点
    pub extra: std::collections::HashSet<NodeId>,
    pub zoom: f64,
    pub center: (f64, f64), // 画布坐标 [0,100]²，默认 (50,50)
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            extra: std::collections::HashSet::new(),
            zoom: 1.0,
            center: (50.0, 50.0),
        }
    }
}

/// TUI 全局状态。
pub struct App {
    pub handle: DbHandle,
    pub path: String,
    /// 查询编辑器缓冲（按字符存储，避免多字节越界）
    pub query: Vec<char>,
    pub cursor: usize,
    pub rows: CliRows,
    /// 与 rows 平行的得分（仅向量检索结果非空；普通 TQL 为空）
    pub row_scores: Vec<f32>,
    pub selected: usize,
    pub detail: Option<CliNode>,
    pub table_state: TableState,
    pub status: String,
    pub last_elapsed: Option<Duration>,
    pub focus: Focus,
    pub left_view: LeftView,
    pub graph_state: GraphState,
    pub show_help: bool,
    pub should_quit: bool,
}

impl App {
    /// `limit` 控制启动默认查询的 LIMIT（来自配置 tui.default_limit）。
    pub fn new(handle: DbHandle, path: String, limit: usize) -> Self {
        // 注意：TQL 不允许空的 `FIND {}`（会报“文档过滤不能为空”），
        // 因此用 Cypher 风格的 `MATCH (n)` 作为“列出全部节点”的默认查询。
        let default_query = format!("MATCH (n) RETURN n LIMIT {limit}");
        App {
            handle,
            path,
            query: default_query.chars().collect(),
            cursor: default_query.chars().count(),
            rows: Vec::new(),
            row_scores: Vec::new(),
            selected: 0,
            detail: None,
            table_state: TableState::default(),
            status: String::from("就绪"),
            last_elapsed: None,
            focus: Focus::Query,
            left_view: LeftView::Results,
            graph_state: GraphState::default(),
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
                    self.row_scores = Vec::new(); // 退出搜索态
                    self.graph_state.extra.clear(); // 新结果集，清空图扩展
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
        // 图视图专属交互键（优先于通用键处理）
        if self.left_view == LeftView::Graph {
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            match key.code {
                KeyCode::Char('e') => {
                    self.graph_expand();
                    return;
                }
                KeyCode::Char('c') => {
                    self.graph_collapse();
                    return;
                }
                KeyCode::Char('f') => {
                    self.graph_reset_view();
                    return;
                }
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.graph_zoom(1.25);
                    return;
                }
                KeyCode::Char('-') | KeyCode::Char('_') => {
                    self.graph_zoom(0.8);
                    return;
                }
                KeyCode::Left if shift => {
                    self.graph_pan(-12.0, 0.0);
                    return;
                }
                KeyCode::Right if shift => {
                    self.graph_pan(12.0, 0.0);
                    return;
                }
                KeyCode::Up if shift => {
                    self.graph_pan(0.0, 12.0);
                    return;
                }
                KeyCode::Down if shift => {
                    self.graph_pan(0.0, -12.0);
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('/') | KeyCode::Tab => self.focus = Focus::Query,
            KeyCode::Char('g') => self.toggle_left_view(),
            KeyCode::Char('s') => self.run_search(),
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev(),
            _ => {}
        }
    }

    /// 以当前选中节点的向量为 query 做相似度检索（搜索 Playground）。
    fn run_search(&mut self) {
        let (vector, qid) = match &self.detail {
            Some(n) => (n.vector.clone(), n.id),
            None => {
                self.status = "无选中节点可作为检索样例".into();
                return;
            }
        };

        let start = Instant::now();
        match self.handle.search_f32(&vector, 20, 1, 0.0) {
            Ok(hits) => {
                let mut rows: CliRows = Vec::new();
                let mut scores: Vec<f32> = Vec::new();
                for (id, score, _payload) in &hits {
                    if let Some(n) = self.handle.get(*id) {
                        let mut row = std::collections::HashMap::new();
                        row.insert("node".to_string(), n);
                        rows.push(row);
                        scores.push(*score);
                    }
                }
                self.rows = rows;
                self.row_scores = scores;
                self.selected = 0;
                self.table_state.select(if self.rows.is_empty() { None } else { Some(0) });
                self.status = format!("SEARCH from #{qid} → {} 命中（Enter 重新查询返回）", self.rows.len());
                self.update_detail();
            }
            Err(e) => self.status = format!("检索错误: {e}"),
        }
        self.last_elapsed = Some(start.elapsed());
    }

    fn toggle_left_view(&mut self) {
        self.left_view = match self.left_view {
            LeftView::Results => LeftView::Graph,
            LeftView::Graph => LeftView::Results,
        };
    }

    /// 把选中节点的 1-hop 邻居加入图（k-hop 展开）。
    fn graph_expand(&mut self) {
        if let Some(id) = self.selected_node_id() {
            for nb in self.handle.neighbors(id, 1) {
                self.graph_state.extra.insert(nb);
            }
            self.status = format!("展开 #{id} 邻居（图扩展节点 {}）", self.graph_state.extra.len());
        }
    }

    /// 折叠所有扩展节点，回到结果集。
    fn graph_collapse(&mut self) {
        self.graph_state.extra.clear();
        self.status = "已折叠图扩展节点".into();
    }

    fn graph_reset_view(&mut self) {
        self.graph_state.zoom = 1.0;
        self.graph_state.center = (50.0, 50.0);
    }

    fn graph_zoom(&mut self, factor: f64) {
        self.graph_state.zoom = (self.graph_state.zoom * factor).clamp(0.3, 5.0);
    }

    fn graph_pan(&mut self, dx: f64, dy: f64) {
        self.graph_state.center.0 += dx / self.graph_state.zoom;
        self.graph_state.center.1 += dy / self.graph_state.zoom;
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
        App::new(h, path, 50)
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
    fn search_by_example_populates_scores() {
        let mut app = temp_app();
        app.initial_load();
        assert!(app.detail.is_some(), "应有选中节点");
        app.run_search();
        assert!(!app.rows.is_empty(), "检索应返回命中");
        assert_eq!(app.rows.len(), app.row_scores.len(), "score 与 rows 应等长");
    }

    #[test]
    fn graph_interaction_zoom_expand_collapse() {
        let mut app = temp_app();
        app.handle.link(1, 2, "knows", 0.9).unwrap(); // 让节点 1 有邻居 2
        app.initial_load();
        app.left_view = LeftView::Graph;

        // 缩放
        let z0 = app.graph_state.zoom;
        app.on_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
        assert!(app.graph_state.zoom > z0, "+ 应放大");

        // 展开选中节点（id=1）的邻居
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert!(!app.graph_state.extra.is_empty(), "展开后应有扩展节点");

        // 折叠
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(app.graph_state.extra.is_empty(), "折叠后扩展节点应清空");
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
