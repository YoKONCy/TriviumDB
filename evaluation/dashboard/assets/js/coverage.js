"use strict";

// 从当前快照计算覆盖，不读取或模拟服务器运行状态。
(() => {
  const app = window.QuiverDashboard;
  const { DATA, $, esc, methods, state, fmt, t, label } = app;

  function renderHealth() {
    const all = DATA.curves.flatMap((curve) =>
      Object.values(curve.points).map((point) => app.model.makeRow(curve, point)),
    );
    const own = all.filter((row) => row.method === "QuIVer");
    const external = all.filter((row) => row.method !== "QuIVer");
    $("healthHero").innerHTML = [
      ["观测记录", all.length],
      ["数据集", DATA.datasets.length],
      ["方法", methods.length],
      ["构建配置", DATA.configs.length],
    ]
      .map(([name, count]) => app.tables.statCard(name, count))
      .join("");
    const entries = [
      [
        "请求 / 库接口 QPS",
        all.filter((row) => row.point.eligible && row.point.qps != null).length,
      ],
      ["P95 原生尾延迟", all.filter((row) => row.p95 != null).length],
      ["P99 原生尾延迟", all.filter((row) => row.p99 != null).length],
      ["逐点驻留快照", all.filter((row) => row.pointResident != null).length],
      ["五轮吞吐样本", all.filter((row) => row.point.samples?.length === 5).length],
    ];
    $("availability").innerHTML = entries
      .map(
        ([name, count]) => `
      <div class="availability-row"><div class="availability-head"><span>${t(name)}</span><b>${fmt(count)} / ${fmt(all.length)}</b></div>
      <div class="availability-track"><i style="width:${(count / all.length) * 100}%"></i></div></div>`,
      )
      .join("");
    const sourceRows = [
      [
        "外部基线",
        t("{datasets} 库 · {count} 观测", {
          datasets: new Set(external.map((row) => row.dataset)).size,
          count: fmt(external.length),
        }),
      ],
      [
        "QuIVer",
        t("{datasets} 库 · {count} 观测", {
          datasets: new Set(own.map((row) => row.dataset)).size,
          count: fmt(own.length),
        }),
      ],
      ["查询参数曲线", t("{count} 组", { count: fmt(DATA.curves.length) })],
      [
        "组件精确账",
        t("{count} / {total} 配置", {
          count: DATA.configs.filter((record) => record.exact).length,
          total: DATA.configs.length,
        }),
      ],
      ["数据快照", DATA.meta.generatedAt?.slice(0, 10) || DATA.meta.updated],
      ["资料导出", DATA.meta.exportedAt?.slice(0, 10) || "—"],
    ];
    $("sourceStatus").innerHTML =
      sourceRows
        .map(
          ([name, value]) =>
            `<div class="source-row"><span>${t(name)}</span><b>${esc(value)}</b></div>`,
        )
        .join("") +
      `<div class="source-row"><span>${t("逐条证据与 SHA-256")}</span><button id="sourceProof">${t("查看备注 ↗")}</button></div>`;
    $("sourceProof").onclick = app.notes.globalNotes;
    const counts = new Map();
    for (const row of all) {
      const key = `${row.dataset}|${row.method}`;
      counts.set(key, (counts.get(key) || 0) + 1);
    }
    $("coverageTable").innerHTML = `
      <thead><tr><th class="left pinned">${t("数据集")}</th>${methods.map((method) => `<th>${esc(label(method))}</th>`).join("")}</tr></thead>
      <tbody>${DATA.datasets
        .map(
          (dataset) => `
        <tr><td class="left pinned">${esc(dataset.name)}</td>${methods
          .map((method) => {
            const count = counts.get(`${dataset.id}|${method}`);
            return `<td class="coverage-cell">${count ? `<button data-cover-ds="${dataset.id}" data-cover-method="${esc(method)}">${count}</button>` : '<span class="null">—</span>'}</td>`;
          })
          .join("")}</tr>`,
        )
        .join("")}</tbody>`;
    $("coverageTable")
      .querySelectorAll("button")
      .forEach((button) => {
        button.onclick = () => {
          state.dataset = button.dataset.coverDs;
          state.recordMethod = button.dataset.coverMethod;
          state.recordFamily = "all";
          state.recordThread = "all";
          state.recordRecall = 0;
          state.recordSearch = "";
          state.view = "explore";
          state.page = 1;
          app.syncRecordInputs();
          app.render();
        };
      });
  }

  Object.assign(app.coverage, { renderHealth });
})();
