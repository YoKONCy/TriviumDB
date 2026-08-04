//! TUI 渲染：标题栏、查询编辑器、结果表、节点详情、状态栏、帮助浮层。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

use super::app::{App, Focus, LeftView};
use super::graph;
use crate::db_handle::{CliNode, CliRows};
use crate::tql_highlight;

pub fn render(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // 标题
        Constraint::Min(0),    // 主体
        Constraint::Length(1), // 状态栏
    ])
    .split(f.area());

    render_title(f, app, chunks[0]);
    render_body(f, app, chunks[1]);
    render_status(f, app, chunks[2]);

    if app.show_help {
        render_help(f, f.area());
    }
}

fn render_title(f: &mut Frame, app: &App, area: Rect) {
    let title = format!(
        " TriviumDB │ {} │ dim={} │ nodes={} │ {} ",
        app.path,
        app.handle.dim(),
        app.handle.node_count(),
        app.handle.dtype(),
    );
    let p = Paragraph::new(title).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(p, area);
}

fn render_body(f: &mut Frame, app: &mut App, area: Rect) {
    let cols =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(area);

    let query_height = (app.lines.len() as u16 + 2).clamp(3, 10); // border(2) + lines
    let left =
        Layout::vertical([Constraint::Length(query_height), Constraint::Min(0)]).split(cols[0]);
    render_query(f, app, left[0]);
    match app.left_view {
        LeftView::Results => render_results(f, app, left[1]),
        LeftView::Graph => graph::render_graph(f, app, left[1], app.focus == Focus::Results),
    }
    render_detail(f, app, cols[1]);
}

fn render_query(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Query;
    let err_style = if app.parse_error_loc.is_some() {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        focus_style(focused)
    };
    let title = if let Some((row, col)) = app.parse_error_loc {
        format!("TQL Query  ✗ line {}, col {}", row + 1, col + 1)
    } else {
        "TQL Query  (Ctrl+Enter 执行)".into()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(err_style);
    // 多行语法高亮 + 错误列下划线
    let text_lines: Vec<Line> = app
        .lines
        .iter()
        .enumerate()
        .map(|(row, l)| {
            let mut spans = tql_highlight::highlight_spans(l);
            if let Some((er_row, er_col)) = app.parse_error_loc
                && er_row == row
            {
                spans = mark_error_column(spans, er_col);
            }
            Line::from(spans)
        })
        .collect();
    let p = Paragraph::new(Text::from(text_lines)).block(block);
    f.render_widget(p, area);

    if focused {
        let max_col = area.width.saturating_sub(2) as usize;
        let x = area.x + 1 + app.cursor_col.min(max_col) as u16;
        let y = area.y + 1 + app.cursor_row as u16;
        if y < area.y + area.height.saturating_sub(1) {
            f.set_cursor_position((x, y));
        }
    }
}

/// 在 spans 序列中把第 `col` 个 unicode scalar 字符标红（包含 underline）。
/// col 超过文本长度时把末尾追加一个红色 caret 字符。
fn mark_error_column<'a>(spans: Vec<Span<'a>>, col: usize) -> Vec<Span<'a>> {
    let err = Style::default()
        .fg(Color::White)
        .bg(Color::Red)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut out: Vec<Span> = Vec::with_capacity(spans.len() + 2);
    let mut consumed = 0usize;
    let mut placed = false;
    for span in spans {
        if placed {
            out.push(span);
            continue;
        }
        let len = span.content.chars().count();
        if consumed + len <= col {
            consumed += len;
            out.push(span);
            continue;
        }
        // 错误列落在本 span 内：拆分
        let local = col - consumed;
        let mut chars = span.content.chars();
        let left: String = chars.by_ref().take(local).collect();
        let mid: String = chars.by_ref().take(1).collect();
        let right: String = chars.collect();
        if !left.is_empty() {
            out.push(Span::styled(left, span.style));
        }
        if !mid.is_empty() {
            out.push(Span::styled(mid, err));
        } else {
            // span 实际只有 left 长度（不应发生，因 len > col-consumed），保险
            out.push(Span::styled("^".to_string(), err));
        }
        if !right.is_empty() {
            out.push(Span::styled(right, span.style));
        }
        placed = true;
    }
    if !placed {
        // 错误列在所有 span 之后（如 EOF 错误），追加一个 caret
        out.push(Span::styled(" ".to_string(), Style::default()));
        out.push(Span::styled("^".to_string(), err));
    }
    out
}

fn render_results(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Results;
    let cols = collect_columns(&app.rows);
    let show_scores = !app.row_scores.is_empty();

    let mut header_cells: Vec<Cell> = vec![Cell::from("#")];
    if show_scores {
        header_cells.push(Cell::from("score"));
    }
    header_cells.extend(cols.iter().map(|c| Cell::from(c.clone())));
    let header = Row::new(header_cells).style(Style::default().add_modifier(Modifier::BOLD));

    let body: Vec<Row> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut cells = vec![Cell::from((i + 1).to_string())];
            if show_scores {
                let s = app.row_scores.get(i).copied().unwrap_or(0.0);
                cells.push(Cell::from(format!("{s:.3}")));
            }
            for c in &cols {
                let text = row.get(c).map(node_cell).unwrap_or_default();
                cells.push(Cell::from(text));
            }
            Row::new(cells)
        })
        .collect();

    let mut widths = vec![Constraint::Length(4)];
    if show_scores {
        widths.push(Constraint::Length(7));
    }
    for _ in &cols {
        widths.push(Constraint::Min(12));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Results ({})", app.rows.len()))
        .border_style(focus_style(focused));

    let table = Table::new(body, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");

    f.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Node Detail");
    let text = detail_text(&app.detail);
    let p = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let hint = match app.focus {
        Focus::Query => "[Ctrl+Enter] 执行  [Enter] 换行  [Tab/Esc] 结果面板  [Ctrl-C] 退出",
        Focus::Results => "[↑/↓] 选择  [/ |Tab] 查询  [g] 图/表  [s] 检索  [?] 帮助  [q] 退出",
    };
    let timing = app
        .last_elapsed
        .map(|d| format!("  ⏱ {d:.2?}"))
        .unwrap_or_default();
    let line = format!(" {hint}  │  {}{}", app.status, timing);
    let p = Paragraph::new(line).style(Style::default().fg(Color::Black).bg(Color::Gray));
    f.render_widget(p, area);
}

fn render_help(f: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 60, area);
    f.render_widget(Clear, popup);

    let lines = vec![
        Line::from(Span::styled(
            "TriviumDB TUI — 快捷键",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("查询编辑器:"),
        Line::from("  Enter        换行"),
        Line::from("  Ctrl+Enter   执行查询（FIND/MATCH/SEARCH 或 CREATE/SET/DELETE）"),
        Line::from("  Tab / Esc    切换到结果面板"),
        Line::from("  ←/→/↑/↓      移动光标   Home/End 行首尾"),
        Line::from("  Backspace    删除字符/合并行   Delete 删除后方/合并下行"),
        Line::from(""),
        Line::from("结果面板:"),
        Line::from("  ↑/↓ 或 j/k   选择行（联动右侧节点详情）"),
        Line::from("  g            切换 结果表格 / 力导向图"),
        Line::from("  s            以选中节点向量做相似度检索（带 score）"),
        Line::from("  / 或 Tab     切换到查询编辑器"),
        Line::from("  ?            显示/隐藏本帮助"),
        Line::from("  q / Esc      退出"),
        Line::from(""),
        Line::from("图视图 (按 g 进入):"),
        Line::from("  e / c        展开选中节点邻居 / 折叠扩展"),
        Line::from("  + / -        缩放    Shift+方向  平移    f  复位视图"),
        Line::from("  m            循环切换字符 (Braille / Dot / Block / HalfBlock)"),
        Line::from(""),
        Line::from("全局:  Ctrl-C 退出"),
        Line::from(""),
        Line::from(Span::styled(
            "按任意键关闭",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let block = Block::default().borders(Borders::ALL).title("帮助 (Help)");
    let p = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(p, popup);
}

// ── 辅助函数 ──

fn focus_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn collect_columns(rows: &CliRows) -> Vec<String> {
    let mut cols: Vec<String> = Vec::new();
    for row in rows {
        for k in row.keys() {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    cols.sort();
    cols
}

fn node_cell(node: &CliNode) -> String {
    let payload = node.payload.to_string();
    let payload = if payload.chars().count() > 80 {
        let s: String = payload.chars().take(80).collect();
        format!("{s}…")
    } else {
        payload
    };
    format!("#{} {}", node.id, payload)
}

fn detail_text(node: &Option<CliNode>) -> Text<'static> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    match node {
        None => Text::from("(未选中节点)"),
        Some(n) => {
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(vec![
                Span::styled("ID: ", bold),
                Span::raw(n.id.to_string()),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Payload:", bold)));
            let payload = serde_json::to_string_pretty(&n.payload).unwrap_or_default();
            for l in payload.lines() {
                lines.push(Line::from(format!("  {l}")));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Edges (out): {}", n.edges.len()),
                bold,
            )));
            for e in &n.edges {
                lines.push(Line::from(format!(
                    "  → {} [{}] w={:.3}",
                    e.target_id, e.label, e.weight
                )));
            }
            lines.push(Line::from(""));
            let dim = n.vector.len();
            let l2 = n.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            let preview: Vec<String> = n.vector.iter().take(8).map(|x| format!("{x:.3}")).collect();
            lines.push(Line::from(Span::styled(
                format!("Vector: dim={dim}, L2={l2:.3}"),
                bold,
            )));
            lines.push(Line::from(format!(
                "  [{}{}]",
                preview.join(", "),
                if dim > 8 { ", …" } else { "" }
            )));
            Text::from(lines)
        }
    }
}

/// 在 `area` 内居中一个 `pct_x% × pct_y%` 的矩形。
fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}
