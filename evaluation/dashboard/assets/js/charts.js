"use strict";
// charts 模块，共享状态由 core.js 提供。
(() => {
  const app = window.QuiverDashboard;
  const { $, esc, ds, state, runtime, fmt, pct, compact, qps, slotInk, letters, t, label } = app;
  function scheduleDraw() {
    if (runtime.drawPending) return;
    runtime.drawPending = true;
    requestAnimationFrame(() => {
      runtime.drawPending = false;
      app.charts.drawChart();
    });
  }

  function chartContext() {
    const canvas = $("battleCanvas"),
      box = canvas.parentElement.getBoundingClientRect(),
      dpr = Math.min(devicePixelRatio || 1, 3),
      w = box.width,
      h = box.height;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    const ctx = canvas.getContext("2d");
    ctx.scale(dpr, dpr);
    const style = getComputedStyle(document.body),
      theme = {
        ink: style.getPropertyValue("--ink").trim(),
        muted: style.getPropertyValue("--muted").trim(),
        grid: style.getPropertyValue("--grid").trim(),
        surface: style.getPropertyValue("--surface").trim(),
      };
    ctx.fillStyle = theme.surface;
    ctx.fillRect(0, 0, w, h);
    ctx.font = '12px "Segoe UI", "Microsoft YaHei", sans-serif';
    runtime.hits = [];
    return { canvas, ctx, w, h, theme };
  }

  function extent(vals, log = false) {
    vals = vals.filter((v) => Number.isFinite(v) && (!log || v > 0));
    if (!vals.length) return log ? [1, 10] : [0, 1];
    let min = Math.min(...vals),
      max = Math.max(...vals);
    if (log) {
      min = Math.log10(min);
      max = Math.log10(max);
      const gap = max - min || 0.5;
      return [10 ** (min - gap * 0.12), 10 ** (max + gap * 0.12)];
    }
    const gap = max - min || Math.max(Math.abs(max) * 0.2, 1);
    return [min - gap * 0.12, max + gap * 0.12];
  }

  function tickValues(min, max, log) {
    if (log) {
      const out = [];
      for (let p = Math.floor(Math.log10(min)); p <= Math.ceil(Math.log10(max)); p++)
        for (const m of [1, 2, 5]) {
          const v = m * 10 ** p;
          if (v >= min && v <= max) out.push(v);
        }
      return out.length > 8 ? out.filter((_, i) => i % 2 === 0) : out;
    }
    const raw = (max - min) / 5,
      power = 10 ** Math.floor(Math.log10(raw || 1)),
      step = ([1, 2, 2.5, 5, 10].find((v) => v * power >= raw) || 10) * power,
      out = [];
    for (let v = Math.ceil(min / step) * step; v <= max + step * 1e-7; v += step) out.push(v);
    return out;
  }

  function axes(ch, points, options) {
    const { ctx, w, h, theme } = ch,
      l = w < 500 ? 58 : 72,
      r = 22,
      t = 28,
      b = 54;
    let xr =
        options.xr ||
        app.charts.extent(
          points.map((p) => p.x),
          options.xlog,
        ),
      yr =
        options.yr ||
        app.charts.extent(
          points.map((p) => p.y),
          options.ylog,
        );
    if (state.zoom) {
      xr = state.zoom.x;
      yr = state.zoom.y;
    }
    const fx = (v) => (options.xlog ? Math.log10(v) : v),
      fy = (v) => (options.ylog ? Math.log10(v) : v),
      ix = (v) => (options.xlog ? 10 ** v : v),
      iy = (v) => (options.ylog ? 10 ** v : v);
    if (!options.ylog && ["recall", "memory"].includes(state.chart)) yr[0] = Math.max(0, yr[0]);
    const X = (v) => l + ((fx(v) - fx(xr[0])) / (fx(xr[1]) - fx(xr[0]))) * (w - l - r),
      Y = (v) => h - b - ((fy(v) - fy(yr[0])) / (fy(yr[1]) - fy(yr[0]))) * (h - t - b);
    runtime.chartBox = { l, r, t, b, w, h, xr, yr, fx, fy, ix, iy, X, Y };
    ctx.strokeStyle = theme.grid;
    ctx.lineWidth = 1;
    ctx.fillStyle = theme.muted;
    ctx.font = '11px "Segoe UI",sans-serif';
    for (const v of options.yticks || app.charts.tickValues(...yr, options.ylog)) {
      const y = Y(v);
      if (y < t || y > h - b) continue;
      ctx.beginPath();
      ctx.moveTo(l, y);
      ctx.lineTo(w - r, y);
      ctx.stroke();
      ctx.textAlign = "right";
      ctx.fillText(compact(v), l - 11, y + 3);
    }
    for (const v of options.xticks || app.charts.tickValues(...xr, options.xlog)) {
      const x = X(v);
      if (x < l || x > w - r) continue;
      ctx.beginPath();
      ctx.moveTo(x, t);
      ctx.lineTo(x, h - b);
      ctx.stroke();
      ctx.textAlign = "center";
      ctx.fillText(options.xpct ? fmt(v, 0) : compact(v), x, h - b + 20);
    }
    ctx.textAlign = "left";
    ctx.fillText(options.ylabel, l, t - 14);
    ctx.textAlign = "right";
    ctx.fillText(options.xlabel, w - r, h - 7);
    return runtime.chartBox;
  }

  function dot(ch, p, a) {
    const { ctx } = ch,
      x = a.X(p.x),
      y = a.Y(p.y);
    if (x < a.l || x > ch.w - a.r || y < a.t || y > ch.h - a.b) return;
    const active = runtime.battleRows.some(
        (r) => r?.curve.id === p.row.curve.id && r.threads === p.row.threads,
      ),
      radius = active ? 6 : 4;
    ctx.globalAlpha = state.hidden.has(p.row.method) ? 0.1 : 1;
    ctx.fillStyle = slotInk(p.row.slot);
    ctx.strokeStyle = ch.theme.surface;
    ctx.lineWidth = 2;
    ctx.beginPath();
    if (p.row.point?.route === "BQ") {
      ctx.moveTo(x, y - radius - 1);
      ctx.lineTo(x + radius + 1, y + radius);
      ctx.lineTo(x - radius - 1, y + radius);
      ctx.closePath();
    } else ctx.arc(x, y, radius, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    if (active) {
      ctx.strokeStyle = slotInk(p.row.slot);
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(x, y, 9, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
    if (!state.hidden.has(p.row.method)) runtime.hits.push({ x, y, radius: 13, row: p.row });
  }

  function drawLines(ch, series, options) {
    const all = series.flatMap((s) => s.points),
      a = app.charts.axes(ch, all, options);
    for (const s of series) {
      const sorted = [...s.points].sort((a, b) => a.x - b.x);
      if (!sorted.length) continue;
      const ctx = ch.ctx;
      ctx.save();
      ctx.beginPath();
      ctx.rect(a.l, a.t, ch.w - a.l - a.r, ch.h - a.t - a.b);
      ctx.clip();
      ctx.strokeStyle = slotInk(s.slot);
      ctx.globalAlpha = state.hidden.has(sorted[0].row.method) ? 0.08 : 1;
      ctx.lineWidth = 2.4;
      ctx.setLineDash(s.dash || []);
      ctx.beginPath();
      sorted.forEach((p, i) =>
        i ? ctx.lineTo(a.X(p.x), a.Y(p.y)) : ctx.moveTo(a.X(p.x), a.Y(p.y)),
      );
      ctx.stroke();
      ctx.restore();
      for (const p of sorted) app.charts.dot(ch, p, a);
    }
    return a;
  }

  function drawBars(ch, rows, kind) {
    const { ctx, w, h, theme } = ch,
      l = Math.min(205, w * 0.37),
      r = 88,
      t = 35,
      b = 45,
      gap = (h - t - b) / Math.max(rows.length, 1),
      barh = Math.min(25, gap * 0.35);
    const values = rows.map((row) =>
        kind === "footprint"
          ? (row.hot || 0) + (row.cold || 0)
          : kind === "latency"
            ? row.p99
            : row.build,
      ),
      max = Math.max(...values.filter((v) => v != null), 1) * 1.12;
    runtime.chartBox = null;
    ctx.fillStyle = theme.muted;
    ctx.font = '12px "Segoe UI",sans-serif';
    ctx.strokeStyle = theme.grid;
    for (const value of app.charts.tickValues(0, max, false)) {
      const x = l + (value / max) * (w - l - r);
      ctx.beginPath();
      ctx.moveTo(x, t - 12);
      ctx.lineTo(x, h - b);
      ctx.stroke();
      ctx.textAlign = "center";
      ctx.fillText(compact(value), x, h - b + 22);
    }
    rows.forEach((row, i) => {
      const y = t + i * gap + gap * 0.32,
        X = (v) => l + ((v || 0) / max) * (w - l - r);
      ctx.textAlign = "right";
      ctx.fillStyle = theme.ink;
      ctx.fillText(
        label(row.method).length > 26 ? label(row.method).slice(0, 25) + "…" : label(row.method),
        l - 15,
        y + barh * 0.8,
      );
      ctx.globalAlpha = state.hidden.has(row.method) ? 0.12 : 1;
      if (kind === "footprint") {
        ctx.fillStyle = slotInk(row.slot);
        ctx.fillRect(l, y, Math.max(0, X(row.hot) - l), barh);
        ctx.globalAlpha *= 0.23;
        ctx.fillRect(
          X(row.hot),
          y,
          Math.max(0, X((row.hot || 0) + (row.cold || 0)) - X(row.hot)),
          barh,
        );
      } else if (kind === "latency") {
        ctx.strokeStyle = slotInk(row.slot);
        ctx.lineWidth = 3;
        ctx.beginPath();
        ctx.moveTo(X(row.p95), y + barh / 2);
        ctx.lineTo(X(row.p99), y + barh / 2);
        ctx.stroke();
        for (const [v, radius] of [
          [row.p95, 5],
          [row.p99, 7],
        ]) {
          ctx.fillStyle = slotInk(row.slot);
          ctx.beginPath();
          ctx.arc(X(v), y + barh / 2, radius, 0, Math.PI * 2);
          ctx.fill();
        }
      } else {
        ctx.fillStyle = slotInk(row.slot);
        ctx.fillRect(l, y, Math.max(0, X(row.build) - l), barh);
      }
      ctx.globalAlpha = 1;
      ctx.textAlign = "left";
      ctx.fillStyle = theme.ink;
      ctx.fillText(fmt(values[i], values[i] < 10 ? 3 : 1), X(values[i]) + 10, y + barh * 0.8);
      if (!state.hidden.has(row.method))
        runtime.hits.push({ x: l, y: y - 10, width: w - l - r, height: barh + 20, row });
    });
    ctx.fillStyle = theme.muted;
    ctx.textAlign = "right";
    ctx.fillText(
      kind === "footprint"
        ? t("热组件 + 逻辑冷 (GiB)")
        : kind === "latency"
          ? t("单查询延迟 (ms)")
          : t("构建时间 (s)"),
      w - r,
      h - 6,
    );
  }

  function drawChart() {
    if (state.view !== "battle") return;
    const ch = app.charts.chartContext(),
      key = state.chart,
      valid = runtime.battleRows.filter(Boolean),
      ylog = state.log;
    $("chartTabs")
      .querySelectorAll("button")
      .forEach((b) => b.classList.toggle("active", b.dataset.chart === key));
    const title = {
      recall: "召回与吞吐",
      memory: "热组件与吞吐",
      scaling: "相同查询点的多线程扩展",
      latency: "当前线程的P95 / P99",
      build: "当前构建配置的时间成本",
      footprint: "持久热组件与逻辑冷数据",
    };
    $("chartHeading").textContent = t(title[key]);
    $("plotScope").textContent =
      `${ds.get(state.dataset)?.name || t("全部库")} · ${state.thread}T · ${t(state.domain === "batch" ? "FAISS批量" : "请求 / 库接口")}`;
    $("logAxis").parentElement.hidden = !["recall", "memory"].includes(key);
    runtime.plotRows = [];
    if (key === "recall") {
      const grouped = new Map();
      state.slots.forEach((s, i) => {
        for (const c of app.model.slotCurves(s)) {
          const p = c.points[String(state.thread)];
          if (!(qps(p) > 0)) continue;
          const row = app.model.makeRow(c, p, i),
            group = s.method + "|" + p.route;
          if (!grouped.has(group))
            grouped.set(group, { slot: i, points: [], dash: p.route === "BQ" ? [4, 4] : [] });
          grouped.get(group).points.push({ x: p.recall * 100, y: qps(p), row });
          runtime.plotRows.push(row);
        }
      });
      const xmin = runtime.plotRows.length
        ? Math.max(
            0,
            Math.floor(Math.min(...runtime.plotRows.map((r) => r.recall * 100)) / 5) * 5 - 2,
          )
        : 80;
      app.charts.drawLines(ch, [...grouped.values()], {
        xr: [xmin, 100.5],
        ylog,
        xpct: true,
        xlabel: "Recall@10 (%)",
        ylabel: "QPS" + (ylog ? " · log" : ""),
      });
    } else if (key === "memory") {
      const points = valid
          .filter((r) => r.qps > 0 && r.hot > 0)
          .map((row) => ({ x: row.hot, y: row.qps, row })),
        a = app.charts.axes(ch, points, {
          xlog: true,
          ylog,
          xlabel: t("热组件小计 (GiB) · log"),
          ylabel: "QPS" + (ylog ? " · log" : ""),
        });
      for (const p of points) app.charts.dot(ch, p, a);
      runtime.plotRows = points.map((p) => p.row);
    } else if (key === "scaling") {
      const series = valid
        .filter((r) => r.q1 > 0)
        .map((r) => ({
          slot: r.slot,
          points: [1, 8, 16, 32]
            .filter((t) => qps(r.curve.points[String(t)]) > 0)
            .map((t) => ({
              x: t,
              y: qps(r.curve.points[String(t)]) / r.q1,
              row: app.model.makeRow(r.curve, r.curve.points[String(t)], r.slot),
            })),
        }));
      const a = app.charts.drawLines(ch, series, {
          xr: [0, 33],
          yr: [0, 34],
          xticks: [1, 8, 16, 32],
          yticks: [0, 8, 16, 24, 32],
          xlabel: t("查询线程"),
          ylabel: t("相对1T加速 (×)"),
        }),
        ctx = ch.ctx;
      ctx.strokeStyle = ch.theme.muted;
      ctx.globalAlpha = 0.35;
      ctx.setLineDash([5, 5]);
      ctx.beginPath();
      ctx.moveTo(a.X(1), a.Y(1));
      ctx.lineTo(a.X(32), a.Y(32));
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
      runtime.plotRows = series.flatMap((s) => s.points.map((p) => p.row));
    } else {
      const eligible = valid.filter((r) =>
        key === "latency"
          ? r.p95 != null && r.p99 != null
          : key === "build"
            ? r.build != null
            : r.hot != null,
      );
      app.charts.drawBars(ch, eligible, key);
      runtime.plotRows = eligible;
    }
    $("chartEmpty").hidden = runtime.plotRows.length > 0;
    $("plottedCount").textContent = t("{count} 个实测点", { count: runtime.plotRows.length });
    $("chartLegend").innerHTML =
      state.slots
        .map(
          (s, i) =>
            `<button data-legend="${esc(s.method)}" class="${state.hidden.has(s.method) ? "muted" : ""}"><i style="background:${slotInk(i)}"></i><strong>${letters[i]}</strong> ${esc(label(s.method))}</button>`,
        )
        .join("") +
      (key === "footprint"
        ? `<span class="subtle">${t("深色：热组件 / 浅色：逻辑冷")}</span>`
        : "");
    $("chartLegend")
      .querySelectorAll("button")
      .forEach(
        (b) =>
          (b.onclick = () => {
            state.hidden.has(b.dataset.legend)
              ? state.hidden.delete(b.dataset.legend)
              : state.hidden.add(b.dataset.legend);
            app.charts.scheduleDraw();
          }),
      );
    $("chartGuidance").textContent = ["recall", "memory", "scaling"].includes(key)
      ? t("悬停查值 · 点击选点 · 滚轮缩放 · 拖动平移")
      : t("悬停查值 · 点击查看观测 · 点击图例聚焦");
    if (runtime.lockedPoint) {
      const r = runtime.plotRows.find(
        (r) =>
          r.curve.id === runtime.lockedPoint.curve.id && r.threads === runtime.lockedPoint.threads,
      );
      if (r) runtime.lockedPoint = r;
      else runtime.lockedPoint = null;
    }
    app.charts.updateInspector(
      runtime.lockedPoint || runtime.hovered || runtime.battleRows.find(Boolean),
      !!runtime.lockedPoint,
    );
  }

  function updateInspector(row, locked = false) {
    if (!row) {
      $("pointInspector").innerHTML =
        `<div class="inspector-idle"><span>◎</span><h3>${t("当前没有观测")}</h3><p>${t("调整数据集、方法或线程以查看已测点。")}</p></div>`;
      $("pointNotes").disabled = true;
      return;
    }
    runtime.hovered = row;
    $("pointState").textContent = t(locked ? "已锁定" : "当前观测");
    $("pointNotes").disabled = false;
    $("pointInspector").innerHTML =
      `<div class="inspector-method" style="color:${slotInk(row.slot) || slotInk(0)}">${esc(label(row.method))}</div><div class="inspector-param">${esc(label(row.params))}<br>${esc(ds.get(row.dataset).name)} · ${row.threads || state.thread}T</div>${[
        ["Recall@10", pct(row.recall)],
        ["QPS", fmt(row.qps)],
        ["热组件 / GiB", fmt(row.hot, 3)],
        ["构建 / s", fmt(row.build, 2)],
        ["P99 / ms", fmt(row.p99, 3)],
        ["实际导航", label(row.point?.route || "—")],
      ]
        .map(([k, v]) => `<div class="inspector-metric"><span>${t(k)}</span><b>${esc(v)}</b></div>`)
        .join("")}`;
  }

  function selectPlotPoint(row) {
    runtime.lockedPoint = row;
    if (["recall", "memory"].includes(state.chart)) {
      state.slots[row.slot].curve = row.curve.id;
      state.paired = false;
      app.battle.renderBattle();
    } else app.charts.updateInspector(row, true);
  }

  function pointAt(e) {
    const b = $("battleCanvas").getBoundingClientRect(),
      x = e.clientX - b.left,
      y = e.clientY - b.top;
    let nearest = null,
      dist = Infinity;
    for (const h of runtime.hits) {
      if (h.width) {
        if (x >= h.x && x <= h.x + h.width && y >= h.y && y <= h.y + h.height) return h.row;
      } else {
        const d = Math.hypot(x - h.x, y - h.y);
        if (d < h.radius && d < dist) {
          dist = d;
          nearest = h.row;
        }
      }
    }
    return nearest;
  }

  $("battleCanvas").onpointerdown = (e) => {
    if (!runtime.chartBox) return;
    runtime.drag = { x: e.clientX, y: e.clientY, box: runtime.chartBox };
    runtime.dragMoved = false;
    $("battleCanvas").setPointerCapture(e.pointerId);
  };

  $("battleCanvas").onpointermove = (e) => {
    if (runtime.drag && e.buttons) {
      const a = runtime.drag.box,
        dx = (e.clientX - runtime.drag.x) / (a.w - a.l - a.r),
        dy = (e.clientY - runtime.drag.y) / (a.h - a.t - a.b),
        sx = a.fx(a.xr[1]) - a.fx(a.xr[0]),
        sy = a.fy(a.yr[1]) - a.fy(a.yr[0]);
      if (Math.abs(e.clientX - runtime.drag.x) + Math.abs(e.clientY - runtime.drag.y) > 4)
        runtime.dragMoved = true;
      state.zoom = {
        x: a.xr.map((v) => a.ix(a.fx(v) - dx * sx)),
        y: a.yr.map((v) => a.iy(a.fy(v) + dy * sy)),
      };
      app.charts.scheduleDraw();
      return;
    }
    const r = app.charts.pointAt(e);
    $("battleCanvas").style.cursor = r ? "pointer" : runtime.chartBox ? "grab" : "default";
    if (r && !runtime.lockedPoint) app.charts.updateInspector(r);
  };

  $("battleCanvas").onpointerup = (e) => {
    const r = !runtime.dragMoved ? app.charts.pointAt(e) : null;
    runtime.drag = null;
    if (r) app.charts.selectPlotPoint(r);
  };

  $("battleCanvas").onwheel = (e) => {
    if (!runtime.chartBox) return;
    e.preventDefault();
    const a = runtime.chartBox,
      box = $("battleCanvas").getBoundingClientRect(),
      px = Math.min(1, Math.max(0, (e.clientX - box.left - a.l) / (a.w - a.l - a.r))),
      py = Math.min(1, Math.max(0, 1 - (e.clientY - box.top - a.t) / (a.h - a.t - a.b))),
      factor = e.deltaY > 0 ? 1.18 : 0.85;
    const zoom = (arr, f, inv, part) => {
      const low = f(arr[0]),
        high = f(arr[1]),
        mid = low + (high - low) * part;
      return [inv(mid + (low - mid) * factor), inv(mid + (high - mid) * factor)];
    };
    state.zoom = { x: zoom(a.xr, a.fx, a.ix, px), y: zoom(a.yr, a.fy, a.iy, py) };
    app.charts.scheduleDraw();
  };

  $("battleCanvas").ondblclick = () => {
    state.zoom = null;
    runtime.lockedPoint = null;
    app.charts.scheduleDraw();
  };
  Object.assign(app.charts, {
    scheduleDraw,
    chartContext,
    extent,
    tickValues,
    axes,
    dot,
    drawLines,
    drawBars,
    drawChart,
    updateInspector,
    selectPlotPoint,
    pointAt,
  });
})();
