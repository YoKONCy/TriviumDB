"use strict";
// tables 模块，共享状态由 core.js 提供。
(() => {
  const app = window.QuiverDashboard;
  const { $, esc, ds, state, runtime, fmt, pct, qps, methodColor, t, label, i18n } = app;
  app.tables.columns = {
    method: ["方法", null, (r) => r.method, "text"],
    dataset: ["数据集", null, (r) => ds.get(r.dataset).name, "text"],
    params: ["查询参数", null, (r) => r.params, "param"],
    buildConfig: ["构建参数", null, (r) => r.buildConfig, "param"],
    recall: ["实测召回", "Recall@10", (r) => r.recall, "pct"],
    threads: ["线程", "T", (r) => r.threads, "number"],
    qps: ["QPS", null, (r) => r.qps, "number"],
    q1: ["1T QPS", null, (r) => r.q1, "number"],
    q8: ["8T QPS", null, (r) => r.q8, "number"],
    q16: ["16T QPS", null, (r) => r.q16, "number"],
    q32: ["32T QPS", null, (r) => r.q32, "number"],
    speed: ["32T / 1T", "×", (r) => r.speed, "ratio"],
    p50: ["P50", "ms", (r) => r.p50, "decimal"],
    p95: ["P95", "ms", (r) => r.p95, "decimal"],
    p99: ["P99", "ms", (r) => r.p99, "decimal"],
    cv: ["五轮CV", null, (r) => r.cv, "pct"],
    hot: ["热组件小计", "GiB", (r) => r.hot, "decimal"],
    cold: ["逻辑冷", "GiB", (r) => r.cold, "decimal"],
    pointResident: ["查询驻留", "GiB", (r) => r.pointResident, "decimal"],
    pss: ["PSS", "GiB", (r) => r.pss, "decimal"],
    resident: ["最大驻留快照", "GiB", (r) => r.resident, "decimal"],
    build: ["构建", "s", (r) => r.build, "decimal"],
    install: ["SM安装", "s", (r) => r.install, "decimal"],
    load: ["加载", "s", (r) => r.load, "decimal"],
    save: ["保存API", "s", (r) => r.save, "decimal"],
    deployment: ["部署文件", "GiB", (r) => r.deployment, "decimal"],
    index: ["索引文件", "GiB", (r) => r.index, "decimal"],
    queries: ["查询数", null, (r) => r.queries, "number"],
    notes: ["备注", null, () => null, "notes"],
  };

  app.tables.battleKeys = {
    core: ["method", "params", "recall", "qps", "hot", "cold", "build", "p99", "notes"],
    full: [
      "method",
      "buildConfig",
      "params",
      "recall",
      "q1",
      "q8",
      "q16",
      "q32",
      "speed",
      "hot",
      "cold",
      "pointResident",
      "build",
      "install",
      "load",
      "p50",
      "p95",
      "p99",
      "cv",
      "deployment",
      "notes",
    ],
  };

  app.tables.columns.cv[2] = (r) => (state.domain === "batch" ? r.point?.batchCv : r.cv);

  app.tables.recordKeys = {
    performance: [
      "method",
      "dataset",
      "params",
      "threads",
      "recall",
      "qps",
      "cv",
      "queries",
      "notes",
    ],
    latency: [
      "method",
      "dataset",
      "params",
      "threads",
      "recall",
      "p50",
      "p95",
      "p99",
      "cv",
      "pointResident",
      "notes",
    ],
    resources: [
      "method",
      "dataset",
      "buildConfig",
      "params",
      "threads",
      "hot",
      "cold",
      "pointResident",
      "resident",
      "pss",
      "deployment",
      "build",
      "install",
      "load",
      "save",
      "notes",
    ],
    full: [
      "method",
      "dataset",
      "buildConfig",
      "params",
      "threads",
      "recall",
      "qps",
      "cv",
      "p50",
      "p95",
      "p99",
      "hot",
      "cold",
      "pointResident",
      "pss",
      "build",
      "install",
      "load",
      "save",
      "index",
      "deployment",
      "queries",
      "notes",
    ],
  };

  function cell(r, key) {
    const [, , get, type] = app.tables.columns[key],
      v = get(r);
    if (type === "notes") return "";
    if (v == null) return '<span class="null">—</span>';
    return type === "pct"
      ? pct(v)
      : type === "ratio"
        ? fmt(v, 2) + "×"
        : type === "decimal"
          ? fmt(v, 3)
          : type === "number"
            ? fmt(v)
            : esc(label(v));
  }

  function paintTable(id, rows, keys, offset = 0) {
    const head =
      "<thead><tr>" +
      keys
        .map(
          (k, i) =>
            `<th data-sort="${k}" class="${["text", "param", "notes"].includes(app.tables.columns[k][3]) ? "left " : ""}${i === 0 ? "pinned " : ""}${k === "notes" ? "pinned-end" : ""}">${t(app.tables.columns[k][0])}${id === "recordsTable" && state.sort === k ? (state.direction < 0 ? " ↓" : " ↑") : ""}${app.tables.columns[k][1] ? `<small>${app.tables.columns[k][1]}</small>` : ""}</th>`,
        )
        .join("") +
      "</tr></thead>";
    $(id).innerHTML =
      head +
      "<tbody>" +
      rows
        .map(
          (r, index) =>
            `<tr class="${r.method === "QuIVer" ? "quiver-row" : ""}">${keys
              .map((k, i) => {
                const kind = app.tables.columns[k][3];
                let content = app.tables.cell(r, k);
                if (k === "method")
                  content = `<span class="table-method"><i style="background:${methodColor(r.method)}"></i>${esc(label(r.method))}${r.point?.route === "BQ" ? '<i class="flag-dot"></i>' : ""}</span>`;
                if (k === "notes")
                  content = `<button class="note-button" data-note="${offset + index}" aria-label="${esc(t("查看 {method} 记录备注", { method: label(r.method) }))}">↗ ${t("备注")}</button>`;
                return `<td class="${["text", "param", "notes"].includes(kind) ? "left " : ""}${i === 0 ? "pinned " : ""}${kind === "param" ? "param " : ""}${kind === "number" ? "number " : ""}${k === "qps" ? "positive " : ""}${k === "notes" ? "pinned-end" : ""}">${content}</td>`;
              })
              .join("")}</tr>`,
        )
        .join("") +
      "</tbody>";
    $(id)
      .querySelectorAll("[data-note]")
      .forEach(
        (b) =>
          (b.onclick = () =>
            app.notes.showRowNotes(
              id === "battleTable" ? rows[+b.dataset.note] : runtime.recordRows[+b.dataset.note],
            )),
      );
    if (id === "recordsTable")
      $(id)
        .querySelectorAll("th[data-sort]")
        .forEach(
          (h) =>
            (h.onclick = () => {
              const k = h.dataset.sort;
              if (k === "notes") return;
              state.direction = state.sort === k ? -state.direction : -1;
              state.sort = k;
              app.tables.renderRecords();
            }),
        );
  }

  function statCard(label, value, detail = "") {
    return `<article class="record-stat"><span>${t(label)}</span><b>${fmt(value)}</b><small>${t(detail)}</small></article>`;
  }

  function renderRecords() {
    runtime.recordRows = app.model.filteredRecords();
    const get = app.tables.columns[state.sort][2];
    runtime.recordRows.sort((a, b) => {
      const x = get(a),
        y = get(b);
      return x == null
        ? y == null
          ? 0
          : 1
        : y == null
          ? -1
          : (typeof x === "number" ? x - y : String(x).localeCompare(String(y))) * state.direction;
    });
    const count = runtime.recordRows.length,
      size = state.pageSize === "all" ? Math.max(count, 1) : +state.pageSize,
      pages = Math.max(1, Math.ceil(count / size));
    state.page = Math.max(1, Math.min(state.page, pages));
    const from = (state.page - 1) * size,
      keys = app.tables.recordKeys[state.recordColumns];
    app.tables.paintTable("recordsTable", runtime.recordRows.slice(from, from + size), keys, from);
    $("recordsEmpty").hidden = count > 0;
    $("recordsTable").hidden = !count;
    $("filterResult").textContent = t("{count} 条匹配", { count: fmt(count) });
    $("recordRange").textContent = count
      ? `${fmt(from + 1)}—${fmt(Math.min(from + size, count))} / ${fmt(count)}`
      : t("0 条");
    $("pageLabel").textContent = state.page + " / " + pages;
    $("previousPage").disabled = state.page === 1;
    $("nextPage").disabled = state.page === pages;
    $("recordsTitle").textContent = t(state.view === "quiver" ? "QuIVer 逐库完整记录" : "全部观测");
    $("recordStats").innerHTML =
      app.tables.statCard("匹配观测", count, "") +
      app.tables.statCard("方法", new Set(runtime.recordRows.map((r) => r.method)).size, "") +
      app.tables.statCard("数据集", new Set(runtime.recordRows.map((r) => r.dataset)).size, "") +
      app.tables.statCard("P99可用", runtime.recordRows.filter((r) => r.p99 != null).length, "");
  }

  function exportRows(rows, keys, name) {
    const csv = [
      keys.map(
        (k) =>
          t(app.tables.columns[k][0]) +
          (app.tables.columns[k][1] ? " (" + app.tables.columns[k][1] + ")" : ""),
      ),
      ...rows.map((r) =>
        keys.map((k) =>
          k === "notes"
            ? Object.entries({ ...r.notes, ...r.point?.notes })
                .filter(([, v]) => v != null && v !== "")
                .map(
                  ([k, v]) =>
                    t(k) +
                    ": " +
                    (typeof v === "object" ? JSON.stringify(i18n.localize(v)) : i18n.localize(v)),
                )
                .join(" | ")
            : label(app.tables.columns[k][2](r) ?? ""),
        ),
      ),
    ]
      .map((row) => row.map((v) => '"' + String(v).replaceAll('"', '""') + '"').join(","))
      .join("\r\n");
    app.download(name, new Blob(["\ufeff" + csv], { type: "text/csv;charset=utf-8" }));
    app.toast(t("已导出 {count} 条记录", { count: fmt(rows.length) }));
  }
  Object.assign(app.tables, { cell, paintTable, statCard, renderRecords, exportRows });
})();
