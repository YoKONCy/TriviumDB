"use strict";
// battle 模块，共享状态由 core.js 提供。
(() => {
  const app = window.QuiverDashboard;
  const {
    DATA,
    $,
    esc,
    methods,
    state,
    runtime,
    fmt,
    pct,
    qps,
    slotInk,
    availability,
    letters,
    shortMethod,
    t,
    label,
  } = app;
  function renderCompetitors() {
    app.model.updateBattleRows();
    $("battleCount").textContent = `${state.slots.length} / 4`;
    $("addMethod").disabled = state.slots.length >= 4;
    $("competitors").innerHTML = state.slots
      .map((slot, index) => {
        const row = runtime.battleRows[index];
        const configurations = DATA.configs.filter(
          (record) => record.dataset === state.dataset && record.method === slot.method,
        );
        const curves = app.model.slotCurves(slot);
        const color = slotInk(index);
        const point = row?.point;
        const variation = state.domain === "batch" ? point?.batchCv : point?.cv;
        const samples = state.domain === "batch" ? point?.batchSamples : point?.samples;
        const methodOptions = methods
          .map((method) => {
            const selected = method === slot.method ? "selected" : "";
            const duplicate = state.slots.some(
              (other, otherIndex) => otherIndex !== index && other.method === method,
            );
            return `<option value="${esc(method)}" ${selected} ${duplicate ? "disabled" : ""}>${esc(label(method))}${availability(method) ? "" : " · " + t("无该库数据")}</option>`;
          })
          .join("");
        const configOptions = configurations
          .map(
            (record) =>
              `<option value="${esc(record.id)}" ${record.id === slot.config ? "selected" : ""}>${esc(label(record.buildConfig))}</option>`,
          )
          .join("");
        const pointOptions = curves
          .map(
            (curve) =>
              `<option value="${esc(curve.id)}" ${curve.id === row?.curve.id ? "selected" : ""}>${esc(label(curve.parameterText))} · ${pct(curve.points[String(state.thread)]?.recall)}</option>`,
          )
          .join("");
        const status = !point
          ? t("{threads}T 未测", { threads: state.thread })
          : slot.curve
            ? t("手动选点")
            : t("预选 {target}", { target: pct(state.target) });
        return `
        <article class="competitor" style="--color:${color}">
          <header class="competitor-top">
            <span class="slot-tag">${letters[index]}</span>
            <select class="method-select" aria-label="${t("方法 {slot}", { slot: letters[index] })}" data-slot-method="${index}">${methodOptions}</select>
            ${index > 1 ? `<button class="remove-slot" data-remove="${index}" aria-label="${t("移除方法")}">×</button>` : ""}
          </header>
          ${
            row
              ? `
            <div class="competitor-metrics">
              <div><div class="metric-label">${t("{threads}T 查询吞吐", { threads: state.thread })}</div><div class="qps-number">${fmt(row.qps)}<small>QPS</small></div></div>
              <div class="recall-box"><div class="metric-label">${t("实测 Recall@10")}</div><div class="recall-number">${pct(row.recall)}</div><span class="status-tag">${status}</span></div>
            </div>
            <details class="competitor-controls">
              <summary>${t("参数配置")}<span class="parameter-preview">${esc(label(row.params))}</span></summary>
              <div class="parameter-grid">
                <label>${t("构建配置")}<select data-slot-config="${index}" aria-label="${t("{slot} 构建配置", { slot: letters[index] })}">${configOptions}</select></label>
                <label>${t("查询点 · 实测召回")}<select data-slot-point="${index}" aria-label="${t("{slot} 查询点", { slot: letters[index] })}">${pointOptions}</select></label>
              </div>
            </details>
            <footer class="competitor-foot">
              <span>${point ? t("{queries} 查询 · {rounds} 轮 · CV {cv}", { queries: fmt(point.queries), rounds: samples?.length || 5, cv: pct(variation) }) : t("该线程未测")}</span>
              <button data-slot-notes="${index}">${t("记录与备注 ↗")}</button>
            </footer>
          `
              : `<div class="empty-card">${t("该方法没有此数据集的测量")}</div>`
          }
        </article>`;
      })
      .join("");
    $("competitors")
      .querySelectorAll("[data-slot-method]")
      .forEach((element) => {
        element.onchange = () => {
          state.slots[+element.dataset.slotMethod] = {
            method: element.value,
            config: null,
            curve: null,
          };
          state.paired = false;
          state.zoom = null;
          runtime.hovered = runtime.lockedPoint = null;
          app.battle.renderBattle();
        };
      });
    $("competitors")
      .querySelectorAll("[data-slot-config]")
      .forEach((element) => {
        element.onchange = () => {
          const slot = state.slots[+element.dataset.slotConfig];
          slot.config = element.value;
          slot.curve = null;
          state.paired = false;
          state.zoom = null;
          runtime.hovered = runtime.lockedPoint = null;
          app.battle.renderBattle();
        };
      });
    $("competitors")
      .querySelectorAll("[data-slot-point]")
      .forEach((element) => {
        element.onchange = () => {
          state.slots[+element.dataset.slotPoint].curve = element.value;
          state.paired = false;
          runtime.hovered = runtime.lockedPoint = null;
          app.battle.renderBattle();
        };
      });
    $("competitors")
      .querySelectorAll("[data-slot-notes]")
      .forEach((element) => {
        element.onclick = () =>
          app.notes.showRowNotes(runtime.battleRows[+element.dataset.slotNotes]);
      });
    $("competitors")
      .querySelectorAll("[data-remove]")
      .forEach((element) => {
        element.onclick = () => {
          state.slots.splice(+element.dataset.remove, 1);
          runtime.hovered = runtime.lockedPoint = null;
          app.battle.renderBattle();
        };
      });
  }

  function alignment() {
    const a = runtime.battleRows[0],
      b = runtime.battleRows[1],
      both = a?.point && b?.point && a.qps != null && b.qps != null,
      gap = both ? Math.abs(a.recall - b.recall) * 100 : null,
      sameQueries = both && a.queries === b.queries,
      sameDomain = both && a.curve.domain === b.curve.domain;
    const close = gap != null && gap <= 0.5,
      valid = both && sameQueries && sameDomain && close;
    const status = !both
      ? t("当前点未形成完整比较")
      : state.paired
        ? t("浏览配对") + " · " + t(close ? "召回接近" : "召回仍有差异")
        : t(close ? "当前点召回接近" : "当前点召回未对齐");
    $("alignmentStatus").innerHTML =
      `<i class="alignment-indicator ${valid ? "ok" : ""}"></i><strong>${status}</strong>${gap != null ? `<span>ΔR ${fmt(gap, 2)} pp</span>` : ""}`;
    $("alignmentChecks").innerHTML =
      `<span class="check-pill ${sameQueries ? "ok" : "warn"}">${t(sameQueries ? "✓ 相同查询数" : "查询覆盖需核对")}</span><span class="check-pill ${sameDomain ? "ok" : "warn"}">${t(sameDomain ? "✓ 相同API类别" : "API边界有差异")}</span><button class="check-pill" id="matchRecall">${t("近似同召回浏览 ⇄")}</button>`;
    $("matchRecall").onclick = app.battle.autoAlign;
  }

  function autoAlign() {
    const aa = app.model
        .slotCurves(state.slots[0])
        .map((c) => app.model.makeRow(c, c.points[String(state.thread)], 0))
        .filter((r) => r.recall >= state.target && r.qps > 0),
      bb = app.model
        .slotCurves(state.slots[1])
        .map((c) => app.model.makeRow(c, c.points[String(state.thread)], 1))
        .filter((r) => r.recall >= state.target && r.qps > 0);
    let best = null;
    for (const a of aa)
      for (const b of bb) {
        const gap = Math.abs(a.recall - b.recall),
          score = gap * 3 + Math.abs((a.recall + b.recall) / 2 - state.target);
        if (!best || score < best.score) best = { a, b, score, gap };
      }
    if (!best) {
      app.toast(t("当前阈值和线程下没有双方共同的实测点"));
      return;
    }
    state.slots[0].curve = best.a.curve.id;
    state.slots[1].curve = best.b.curve.id;
    state.paired = true;
    app.battle.renderBattle();
    app.toast(t("已选择实测点，召回差 {gap} pp", { gap: fmt(best.gap * 100, 2) }));
  }

  function renderDeltas() {
    const rows = runtime.battleRows.slice(0, 2),
      metrics = [
        ["查询吞吐", "qps", "QPS", 0, true, "吞吐优势"],
        ["热组件小计", "hot", "GiB", 3, false, "内存效率"],
        ["构建时间", "build", "s", 2, false, "构建效率"],
        ["P99 延迟", "p99", "ms", 3, false, "延迟优势"],
      ];
    $("deltaGrid").innerHTML = metrics
      .map(([title, key, unit, d, higher, ratioLabel]) => {
        const x = rows[0]?.[key],
          y = rows[1]?.[key],
          result = app.model.metricAdvantage(x, y, higher),
          color = result.winner == null ? "var(--muted)" : slotInk(result.winner);
        const ratioText = !result.valid
          ? "—"
          : Number.isFinite(result.ratio)
            ? fmt(result.ratio, 2)
            : "∞";
        const leader = !result.valid
          ? t("缺少可比数据")
          : result.tie
            ? t("双方持平")
            : `${esc(shortMethod(rows[result.winner].method))} <strong>${Number.isFinite(result.percent) ? t("领先 {percent}", { percent: fmt(result.percent, 2) + "%" }) : t("幅度不可计算")}</strong>`;
        return `<article class="delta-card ${result.valid ? "" : "delta-unavailable"}" data-metric="${key}" style="--advantage:${color}">
   <div class="delta-title"><span>${t(title)}</span><span>${t(higher ? "越高越好 ↑" : "越低越好 ↓")}</span></div>
   <div class="delta-value">${ratioText}<small>× ${t(ratioLabel)}</small></div>
   <div class="delta-detail"><span><i style="background:${slotInk(0)}"></i>A ${fmt(x, d)}</span><span><i style="background:${slotInk(1)}"></i>B ${fmt(y, d)} ${unit}</span></div>
   <div class="delta-bars" role="img" aria-label="${esc(t("{metric} 优势占比", { metric: t(title) }))} · ${result.valid ? `A ${fmt(result.shareA, 1)}%, B ${fmt(result.shareB, 1)}%` : t("数据缺失")}">${result.valid ? `<i style="width:${result.shareA}%;background:${slotInk(0)}"></i><i style="width:${result.shareB}%;background:${slotInk(1)}"></i>` : ""}</div>
   <div class="delta-lead">${leader}</div>
  </article>`;
      })
      .join("");
  }

  function renderBattle() {
    app.battle.renderCompetitors();
    app.battle.alignment();
    app.battle.renderDeltas();
    app.tables.paintTable(
      "battleTable",
      runtime.battleRows.filter(Boolean),
      app.tables.battleKeys[state.columns],
    );
    app.charts.scheduleDraw();
  }
  Object.assign(app.battle, {
    renderCompetitors,
    alignment,
    autoAlign,
    renderDeltas,
    renderBattle,
  });
})();
