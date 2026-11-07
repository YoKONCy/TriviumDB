//! TUI 应用状态与事件处理。

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::symbols::Marker;
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
    /// 查询编辑器：多行文本缓冲
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
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
    /// 最近一次解析错误在查询编辑器中的 (line, col)（0-based）
    pub parse_error_loc: Option<(usize, usize)>,
    /// 图视图字符渲染模式（Braille / Dot / Block / HalfBlock）
    pub graph_marker: Marker,
}

impl App {
    /// `limit` 控制启动默认查询的 LIMIT（来自配置 tui.default_limit）。
    /// `marker` 决定图视图使用的字符（Braille 默认；旧 Windows 控制台降级为 Dot）。
    pub fn new(handle: DbHandle, path: String, limit: usize, marker: Marker) -> Self {
        // 注意：TQL 不允许空的 `FIND {}`（会报“文档过滤不能为空”），
        // 因此用 Cypher 风格的 `MATCH (n)` 作为“列出全部节点”的默认查询。
        let default_query = format!("MATCH (n) RETURN n LIMIT {limit}");
        App {
            handle,
            path,
            lines: vec![default_query.clone()],
            cursor_row: 0,
            cursor_col: default_query.chars().count(),
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
            parse_error_loc: None,
            graph_marker: marker,
        }
    }

    pub fn query_string(&self) -> String {
        self.lines.join("\n")
    }

    /// 当前行字符数
    fn current_line_len(&self) -> usize {
        self.lines[self.cursor_row].chars().count()
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
            self.parse_error_loc = None;
            return;
        }

        let start = Instant::now();
        // 预校验：带位置的解析错误高亮
        let parse_res = if is_mutation(&q) {
            triviumdb::query::tql_parser::parse_tql_statement_with_pos(&q).map(|_| ())
        } else {
            triviumdb::query::tql_parser::parse_tql_with_pos(&q).map(|_| ())
        };
        if let Err(err) = parse_res {
            let diag = crate::diagnostics::Diagnostic::from_parse_error(&q, &err);
            self.parse_error_loc = Some((diag.line, diag.col));
            self.status = format!(
                "解析错误 line {}, col {}: {}",
                diag.line + 1,
                diag.col + 1,
                diag.msg
            );
            self.last_elapsed = Some(start.elapsed());
            return;
        }
        self.parse_error_loc = None;
        if is_mutation(&q) {
            match self.handle.tql_mut(&q) {
                Ok(s) => {
                    self.status = match self.handle.flush() {
                        Ok(_) => format!(
                            "写入成功 affected={}, created_ids={:?}",
                            s.affected, s.created_ids
                        ),
                        Err(e) => format!(
                            "写入成功但 flush 失败: {e}; affected={}, created_ids={:?}",
                            s.affected, s.created_ids
                        ),
                    };
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
                    self.table_state
                        .select(if self.rows.is_empty() { None } else { Some(0) });
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
        self.detail = match id.map(|id| self.handle.get(id)).transpose() {
            Ok(node) => node.flatten(),
            Err(error) => {
                self.status = format!("读取节点失败：{error}");
                None
            }
        };
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter if ctrl => self.execute_query(),
            KeyCode::Enter => {
                // 在当前行光标处断行
                let line: Vec<char> = self.lines[self.cursor_row].chars().collect();
                let left: String = line[..self.cursor_col].iter().collect();
                let right: String = line[self.cursor_col..].iter().collect();
                self.lines[self.cursor_row] = left;
                self.lines.insert(self.cursor_row + 1, right);
                self.cursor_row += 1;
                self.cursor_col = 0;
            }
            KeyCode::Tab => self.focus = Focus::Results,
            KeyCode::Esc => self.focus = Focus::Results,
            KeyCode::Backspace => {
                if self.cursor_col > 0 {
                    let mut chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
                    chars.remove(self.cursor_col - 1);
                    self.lines[self.cursor_row] = chars.into_iter().collect();
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    // 合并到上一行
                    let current = self.lines.remove(self.cursor_row);
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].chars().count();
                    self.lines[self.cursor_row].push_str(&current);
                }
            }
            KeyCode::Delete => {
                let len = self.current_line_len();
                if self.cursor_col < len {
                    let mut chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
                    chars.remove(self.cursor_col);
                    self.lines[self.cursor_row] = chars.into_iter().collect();
                } else if self.cursor_row + 1 < self.lines.len() {
                    // 合并下一行
                    let next = self.lines.remove(self.cursor_row + 1);
                    self.lines[self.cursor_row].push_str(&next);
                }
            }
            KeyCode::Left => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.current_line_len();
                }
            }
            KeyCode::Right => {
                let len = self.current_line_len();
                if self.cursor_col < len {
                    self.cursor_col += 1;
                } else if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = 0;
                }
            }
            KeyCode::Up => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.cursor_col.min(self.current_line_len());
                }
            }
            KeyCode::Down => {
                if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = self.cursor_col.min(self.current_line_len());
                }
            }
            KeyCode::Home => self.cursor_col = 0,
            KeyCode::End => self.cursor_col = self.current_line_len(),
            KeyCode::Char(c) => {
                let mut chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
                chars.insert(self.cursor_col, c);
                self.lines[self.cursor_row] = chars.into_iter().collect();
                self.cursor_col += 1;
                self.parse_error_loc = None;
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
                KeyCode::Char('m') => {
                    self.graph_marker = super::marker::GraphMarker::cycle(self.graph_marker);
                    self.status = format!(
                        "图字符: {}",
                        super::marker::GraphMarker::label(self.graph_marker)
                    );
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
                    if let Some(n) = match self.handle.get(*id) {
                        Ok(node) => node,
                        Err(error) => {
                            self.status = format!("读取节点失败：{error}");
                            return;
                        }
                    } {
                        let mut row = std::collections::HashMap::new();
                        row.insert("node".to_string(), n);
                        rows.push(row);
                        scores.push(*score);
                    }
                }
                self.rows = rows;
                self.row_scores = scores;
                self.selected = 0;
                self.table_state
                    .select(if self.rows.is_empty() { None } else { Some(0) });
                self.status = format!(
                    "SEARCH from #{qid} → {} 命中（Enter 重新查询返回）",
                    self.rows.len()
                );
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
            for nb in match self.handle.neighbors(id, 1) {
                Ok(nodes) => nodes,
                Err(error) => {
                    self.status = format!("读取邻居失败：{error}");
                    return;
                }
            } {
                self.graph_state.extra.insert(nb);
            }
            self.status = format!(
                "展开 #{id} 邻居（图扩展节点 {}）",
                self.graph_state.extra.len()
            );
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
