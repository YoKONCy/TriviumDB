//! Graph View：把当前结果集的节点 + 边渲染为 ASCII 力导向图。
//!
//! 用 Fruchterman-Reingold 布局算法计算 2D 坐标，再用 ratatui 的 `Canvas`
//! （Braille 标记画边、`ctx.print` 画节点）渲染。结果集通常 ≤ 50 个节点，
//! O(n²) 的 FR 完全够用。

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use triviumdb::node::NodeId;

use super::app::App;
use crate::db_handle::{CliNode, CliRow};

struct GraphData {
    labels: Vec<String>,
    is_extra: Vec<bool>, // 是否为 k-hop 展开加入的节点
    edges: Vec<(usize, usize)>,
    pos: Vec<(f64, f64)>, // 归一化到 [0,1]
    selected: Option<usize>,
}

pub fn render_graph(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let data = build_graph(app);

    let border_style = if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let z = app.graph_state.zoom;
    let marker_name = super::marker::GraphMarker::label(app.graph_marker);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            "Graph ({}n {}e) {z:.1}x [{marker_name}]  [e/c]展开折叠 [+/-/Shift方向/f]视图 [m]字符 [g]表格",
            data.labels.len(),
            data.edges.len()
        ))
        .border_style(border_style);

    if data.labels.is_empty() {
        f.render_widget(Paragraph::new("(当前结果无可视化节点)").block(block), area);
        return;
    }

    // 力导向坐标 [0,1] → 画布 [0,100]（画布 y 轴向上，故翻转）
    let xy: Vec<(f64, f64)> = data
        .pos
        .iter()
        .map(|&(x, y)| (x * 100.0, (1.0 - y) * 100.0))
        .collect();
    let edges = data.edges.clone();
    let labels = data.labels.clone();
    let is_extra = data.is_extra.clone();
    let selected = data.selected;

    // 视口：以 center 为中心，半宽 50/zoom
    let (cx, cy) = app.graph_state.center;
    let half = 50.0 / z;

    let canvas = Canvas::default()
        .block(block)
        .marker(app.graph_marker)
        .x_bounds([cx - half, cx + half])
        .y_bounds([cy - half, cy + half])
        .paint(move |ctx| {
            for &(a, b) in &edges {
                ctx.draw(&CanvasLine {
                    x1: xy[a].0,
                    y1: xy[a].1,
                    x2: xy[b].0,
                    y2: xy[b].1,
                    color: Color::DarkGray,
                });
            }
            ctx.layer(); // 节点画在边之上
            for (i, &(x, y)) in xy.iter().enumerate() {
                let style = if selected == Some(i) {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if is_extra[i] {
                    Style::default().fg(Color::Green) // 展开节点
                } else {
                    Style::default().fg(Color::Cyan) // 结果集节点
                };
                ctx.print(x, y, Span::styled(format!("●{}", labels[i]), style));
            }
        });

    f.render_widget(canvas, area);
}

fn build_graph(app: &App) -> GraphData {
    use std::collections::hash_map::Entry;

    let mut order: Vec<NodeId> = Vec::new();
    let mut label_of: HashMap<NodeId, String> = HashMap::new();
    let mut extra_of: HashMap<NodeId, bool> = HashMap::new();

    // 1) 结果集节点
    for row in &app.rows {
        let mut keys: Vec<&String> = row.keys().collect();
        keys.sort();
        for k in keys {
            let node = &row[k];
            if let Entry::Vacant(e) = label_of.entry(node.id) {
                order.push(node.id);
                e.insert(node_label_node(node));
                extra_of.insert(node.id, false);
            }
        }
    }

    // 2) k-hop 展开加入的额外节点（结果集之外）
    for &id in &app.graph_state.extra {
        if let Entry::Vacant(e) = label_of.entry(id) {
            order.push(id);
            let name = app
                .handle
                .get_payload(id)
                .and_then(|p| p.get("name").and_then(|v| v.as_str()).map(str::to_string))
                .unwrap_or_else(|| format!("#{id}"));
            e.insert(truncate_label(&name));
            extra_of.insert(id, true);
        }
    }

    let index: HashMap<NodeId, usize> =
        order.iter().enumerate().map(|(i, &id)| (id, i)).collect();

    // 2) 收集结果集内部的边（两端都在结果集中）
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for (i, &id) in order.iter().enumerate() {
        for e in app.handle.get_edges(id) {
            if let Some(&j) = index.get(&e.target_id) {
                let key = if i < j { (i, j) } else { (j, i) };
                if i != j && seen.insert(key) {
                    edges.push((i, j));
                }
            }
        }
    }

    // 3) 力导向布局
    let mut pos = fr_layout(order.len(), &edges, 120);
    normalize(&mut pos);

    // 4) 选中节点下标
    let selected = primary_id(app).and_then(|id| index.get(&id).copied());

    let labels: Vec<String> = order.iter().map(|id| label_of[id].clone()).collect();
    let is_extra: Vec<bool> = order.iter().map(|id| extra_of[id]).collect();
    GraphData { labels, is_extra, edges, pos, selected }
}

/// 节点标签：优先 payload.name，否则 #id；截断到 ~12 字符。
fn node_label_node(node: &CliNode) -> String {
    let name = node
        .payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("#{}", node.id));
    truncate_label(&name)
}

fn truncate_label(name: &str) -> String {
    if name.chars().count() > 12 {
        let s: String = name.chars().take(12).collect();
        format!("{s}…")
    } else {
        name.to_string()
    }
}

/// 选中行的主节点 id（首个字典序变量的节点）。
fn primary_id(app: &App) -> Option<NodeId> {
    let row = app.rows.get(app.selected)?;
    let mut keys: Vec<&String> = row.keys().collect();
    keys.sort();
    Some(row.get(*keys.first()?)?.id)
}

/// Fruchterman-Reingold 力导向布局，返回 [0,1]² 内的坐标。
fn fr_layout(n: usize, edges: &[(usize, usize)], iters: usize) -> Vec<(f64, f64)> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(0.5, 0.5)];
    }

    // 初始化在圆周上（确定性）
    let mut pos: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            (0.5 + 0.4 * a.cos(), 0.5 + 0.4 * a.sin())
        })
        .collect();

    let k = (1.0 / n as f64).sqrt(); // 理想边长
    let mut temp = 0.1_f64;

    for _ in 0..iters {
        let mut disp = vec![(0.0_f64, 0.0_f64); n];

        // 斥力（所有节点对）
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let dx = pos[i].0 - pos[j].0;
                let dy = pos[i].1 - pos[j].1;
                let dist = (dx * dx + dy * dy).sqrt().max(1e-4);
                let force = k * k / dist;
                disp[i].0 += dx / dist * force;
                disp[i].1 += dy / dist * force;
            }
        }

        // 引力（边）
        for &(a, b) in edges {
            let dx = pos[a].0 - pos[b].0;
            let dy = pos[a].1 - pos[b].1;
            let dist = (dx * dx + dy * dy).sqrt().max(1e-4);
            let force = dist * dist / k;
            let (fx, fy) = (dx / dist * force, dy / dist * force);
            disp[a].0 -= fx;
            disp[a].1 -= fy;
            disp[b].0 += fx;
            disp[b].1 += fy;
        }

        // 按温度限幅施加位移
        for i in 0..n {
            let d = (disp[i].0 * disp[i].0 + disp[i].1 * disp[i].1).sqrt().max(1e-4);
            pos[i].0 = (pos[i].0 + disp[i].0 / d * d.min(temp)).clamp(0.0, 1.0);
            pos[i].1 = (pos[i].1 + disp[i].1 / d * d.min(temp)).clamp(0.0, 1.0);
        }
        temp *= 0.95; // 退火
    }

    pos
}

/// 把坐标重新缩放到铺满 [0,1]²。
fn normalize(pos: &mut [(f64, f64)]) {
    if pos.len() < 2 {
        return;
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in pos.iter() {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    let w = (maxx - minx).max(1e-6);
    let h = (maxy - miny).max(1e-6);
    for p in pos.iter_mut() {
        p.0 = (p.0 - minx) / w;
        p.1 = (p.1 - miny) / h;
    }
}

/// 给定一行结果，返回其主节点 id（用于结果排序）。
pub fn row_primary_id(row: &CliRow) -> NodeId {
    let mut keys: Vec<&String> = row.keys().collect();
    keys.sort();
    keys.first()
        .and_then(|k| row.get(*k))
        .map(|n| n.id)
        .unwrap_or(NodeId::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_layout_stays_in_unit_square() {
        let pos = fr_layout(5, &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 0)], 80);
        assert_eq!(pos.len(), 5);
        for (x, y) in pos {
            assert!((0.0..=1.0).contains(&x), "x out of range: {x}");
            assert!((0.0..=1.0).contains(&y), "y out of range: {y}");
        }
    }

    #[test]
    fn fr_layout_edge_cases() {
        assert!(fr_layout(0, &[], 10).is_empty());
        assert_eq!(fr_layout(1, &[], 10), vec![(0.5, 0.5)]);
    }

    #[test]
    fn row_primary_id_picks_first_sorted_var() {
        let mk = |id| CliNode {
            id,
            vector: vec![],
            payload: serde_json::Value::Null,
            edges: vec![],
        };
        let mut row: CliRow = std::collections::HashMap::new();
        row.insert("b".to_string(), mk(20));
        row.insert("a".to_string(), mk(10));
        // "a" 字典序在前 → 主 id = 10
        assert_eq!(row_primary_id(&row), 10);
    }
}
