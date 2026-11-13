"use strict";
// model 模块，共享状态由 core.js 提供。
(() => {
  const app = window.QuiverDashboard;
  const { DATA, ds, cfg, state, runtime, qps, i18n } = app;
  function makeRow(c, p = null, slot = -1) {
    const b = cfg.get(c.config);
    return {
      ...b,
      curve: c,
      point: p,
      dataset: c.dataset,
      method: c.method,
      params: c.parameterText,
      threads: p?.threads,
      recall: p?.recall,
      qps: qps(p),
      q1: qps(c.points["1"]),
      q8: qps(c.points["8"]),
      q16: qps(c.points["16"]),
      q32: qps(c.points["32"]),
      speed:
        qps(c.points["1"]) > 0 && qps(c.points["32"]) != null
          ? qps(c.points["32"]) / qps(c.points["1"])
          : null,
      // 批量吞吐没有对应的单请求尾数组，保留原记录并在此视图留空。
      p50: state.domain === "batch" ? null : p?.p50,
      p95: state.domain === "batch" ? null : p?.p95,
      p99: state.domain === "batch" ? null : p?.p99,
      cv: state.domain === "batch" ? null : p?.cv,
      pointResident: p?.resident,
      pss: p?.pss,
      queries: p?.queries,
      slot,
    };
  }

  function slotCurves(s) {
    return DATA.curves.filter(
      (c) =>
        c.dataset === state.dataset &&
        c.method === s.method &&
        (!s.config || c.config === s.config),
    );
  }

  function chooseSlot(s, i) {
    let all = DATA.curves.filter((c) => c.dataset === state.dataset && c.method === s.method);
    if (!all.length) return null;
    if (!s.config || !all.some((c) => c.config === s.config)) {
      const anchor = all.find((c) =>
        c.targets.some((t) => t.target != null && Math.abs(t.target - state.target) < 1e-8),
      );
      s.config = (anchor || all[0]).config;
      s.curve = null;
    }
    all = all.filter((c) => c.config === s.config);
    let c = all.find((c) => c.id === s.curve);
    if (!c)
      c =
        all.find((c) =>
          c.targets.some((t) => t.target != null && Math.abs(t.target - state.target) < 1e-8),
        ) ||
        all.find((c) => c.targets.some((t) => t.target == null)) ||
        all[all.length - 1];
    return app.model.makeRow(c, c.points[String(state.thread)] || null, i);
  }

  function updateBattleRows() {
    runtime.battleRows = state.slots.map(app.model.chooseSlot);
  }

  function metricAdvantage(x, y, higher) {
    if (x == null || y == null || !Number.isFinite(x) || !Number.isFinite(y) || x < 0 || y < 0)
      return { valid: false };
    const tie = x === y,
      winner = tie ? null : (higher ? x > y : x < y) ? 0 : 1,
      best = winner === 1 ? y : x,
      worst = winner === 1 ? x : y;
    const ratio = tie ? 1 : higher ? best / worst : worst / best;
    const percent = tie
      ? 0
      : higher
        ? ((best - worst) / worst) * 100
        : ((worst - best) / worst) * 100;
    // 低成本指标的优势权重取倒数；等价形式避免零值除法。
    const total = x + y,
      shareA = total ? ((higher ? x : y) / total) * 100 : 50;
    return { valid: true, tie, winner, ratio, percent, shareA, shareB: 100 - shareA };
  }

  function filteredRecords() {
    const out = [];
    for (const c of DATA.curves) {
      if (state.dataset !== "all" && c.dataset !== state.dataset) continue;
      if (state.view === "quiver" && c.method !== "QuIVer") continue;
      if (state.recordMethod !== "all" && c.method !== state.recordMethod) continue;
      if (state.recordFamily !== "all" && c.family !== state.recordFamily) continue;
      if (
        state.recordSearch &&
        ![
          c.method,
          c.family,
          ds.get(c.dataset).name,
          c.parameterText,
          cfg.get(c.config).buildConfig,
        ]
          .map(i18n.searchText)
          .join(" ")
          .toLowerCase()
          .includes(state.recordSearch.toLowerCase())
      )
        continue;
      for (const p of Object.values(c.points)) {
        if (
          (state.recordThread !== "all" && p.threads !== +state.recordThread) ||
          p.recall < state.recordRecall / 100 ||
          (state.domain === "batch" && p.batch == null)
        )
          continue;
        out.push(app.model.makeRow(c, p));
      }
    }
    return out;
  }
  Object.assign(app.model, {
    makeRow,
    slotCurves,
    chooseSlot,
    updateBattleRows,
    metricAdvantage,
    filteredRecords,
  });
})();
