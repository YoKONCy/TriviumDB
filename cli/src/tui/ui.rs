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
    let cols = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(area);

    let left = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(cols[0]);
    render_query(f, app, left[0]);
    match app.left_view {
        LeftView::Results => render_results(f, app, left[1]),
        LeftView::Graph => graph::render_graph(f, app, left[1], app.focus == Focus::Results),
    }
    render_detail(f, app, cols[1]);
}

fn render_query(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Query;
    let block = Block::default()
        .borders(Borders::ALL)
        .title("TQL Query  (Enter 执行)")
        .border_style(focus_style(focused));
    // 语法高亮：把查询渲染为彩色 Span
    let spans = tql_highlight::highlight_spans(&app.query_string());
    let p = Paragraph::new(Line::from(spans)).block(block);
    f.render_widget(p, area);

    if focused {
        // 把终端光标放到编辑位置（字符列近似为列偏移）
        let x = area.x + 1 + app.cursor.min(area.width.saturating_sub(2) as usize) as u16;
        let y = area.y + 1;
        f.set_cursor_position((x, y));
    }
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
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Node Detail");
    let text = detail_text(&app.detail);
    let p = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let hint = match app.focus {
        Focus::Query => "[Enter] 执行  [Tab/Esc] 结果面板  [Ctrl-C] 退出",
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
        Line::from("  Enter        执行查询（FIND/MATCH/SEARCH 或 CREATE/SET/DELETE）"),
        Line::from("  Tab / Esc    切换到结果面板"),
        Line::from("  ←/→ Home/End 移动光标   Backspace/Delete 删除"),
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
        Line::from(""),
        Line::from("全局:  Ctrl-C 退出"),
        Line::from(""),
        Line::from(Span::styled("按任意键关闭", Style::default().fg(Color::DarkGray))),
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
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
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
