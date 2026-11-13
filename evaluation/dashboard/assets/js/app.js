"use strict";
// app 模块，共享状态由 core.js 提供。
(() => {
  const app = window.QuiverDashboard;
  const {
    DATA,
    $,
    esc,
    ds,
    cfg,
    curveById,
    methods,
    state,
    runtime,
    fmt,
    availability,
    i18n,
    t,
    label,
  } = app;
  function renderScenario() {
    $("dataset").value = state.dataset;
    $("target").value = String(state.target);
    $("domain").value = state.domain;
    const d = ds.get(state.dataset);
    $("datasetMeta").textContent = d
      ? `${fmt(d.n)} × ${d.dim}d · ${d.metric === "euclidean" ? t("原生L2") : "Cosine"}`
      : t("{count} 个数据集 / 全部度量", { count: DATA.datasets.length });
    $("threads")
      .querySelectorAll("button")
      .forEach((b) => b.classList.toggle("active", +b.dataset.thread === state.thread));
    $("navigation")
      .querySelectorAll("button")
      .forEach((b) => b.classList.toggle("active", b.dataset.view === state.view));
    const titles = {
      battle: "方法对比",
      explore: "全量记录",
      quiver: "QuIVer 全库",
      health: "覆盖与来源",
    };
    $("pageTitle").textContent = t(titles[state.view]);
    $("viewTitle").textContent = t(titles[state.view]);
    $("battleView").hidden = state.view !== "battle";
    $("exploreView").hidden = !["explore", "quiver"].includes(state.view);
    $("healthView").hidden = state.view !== "health";
    $("threadField").hidden = state.view !== "battle";
    $("targetField").hidden = state.view !== "battle";
    $("scenePanel").hidden = state.view === "health";
    $("recordMethod").disabled = state.view === "quiver";
    $("totalPoints").textContent = fmt(DATA.meta.points);
    const generated = DATA.meta.exportedAt || DATA.meta.generatedAt;
    $("snapshotTime").textContent = generated
      ? t("资料更新 {date}", { date: generated.slice(0, 10) })
      : t("离线数据快照");
    $("buildStamp").textContent = generated ? generated.slice(0, 10) : t("离线数据");
  }

  function render() {
    app.renderScenario();
    if (state.view === "battle") app.battle.renderBattle();
    else if (state.view === "health") app.coverage.renderHealth();
    else app.tables.renderRecords();
  }

  function toast(message) {
    $("toast").textContent = message;
    $("toast").hidden = false;
    setTimeout(() => ($("toast").hidden = true), 2900);
  }

  function download(name, blob) {
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = name;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 3000);
  }

  function resetScene() {
    Object.assign(state, {
      dataset: "D03",
      thread: 1,
      target: 0.95,
      domain: "request",
      slots: [
        { method: "QuIVer", config: null, curve: null },
        { method: "Lucene BBQ + FP32", config: null, curve: null },
      ],
      paired: false,
      hidden: new Set(),
      zoom: null,
      recordSearch: "",
      recordMethod: state.view === "quiver" ? "QuIVer" : "all",
      recordFamily: "all",
      recordThread: "all",
      recordRecall: 0,
      page: 1,
    });
    runtime.hovered = runtime.lockedPoint = null;
    app.syncRecordInputs();
    app.render();
  }

  function syncRecordInputs() {
    for (const [id, key] of [
      ["recordSearch", "recordSearch"],
      ["recordMethod", "recordMethod"],
      ["recordFamily", "recordFamily"],
      ["recordThread", "recordThread"],
      ["recordRecall", "recordRecall"],
    ])
      $(id).value = state[key];
  }

  function setView(view) {
    state.view = view;
    state.page = 1;
    runtime.hovered = runtime.lockedPoint = null;
    if (view === "battle") {
      if (state.dataset === "all") state.dataset = "D03";
    } else {
      state.dataset = "all";
      state.recordSearch = "";
      state.recordMethod = view === "quiver" ? "QuIVer" : "all";
      state.recordFamily = "all";
      state.recordThread = "all";
      state.recordRecall = 0;
      app.syncRecordInputs();
    }
    app.render();
  }

  // 先公开跨模块入口，再绑定事件并执行首次渲染。
  Object.assign(app, {
    renderScenario,
    render,
    toast,
    download,
    resetScene,
    syncRecordInputs,
    setView,
  });

  function populateOptions() {
    $("dataset").innerHTML =
      `<option value="all">${t("全部数据集")}</option>` +
      [...new Set(DATA.datasets.map((d) => d.group))]
        .map(
          (g) =>
            `<optgroup label="${esc(label(g))}">` +
            DATA.datasets
              .filter((d) => d.group === g)
              .map((d) => `<option value="${d.id}">${esc(label(d.name))} · ${d.dim}d</option>`)
              .join("") +
            "</optgroup>",
        )
        .join("");

    $("recordMethod").innerHTML =
      `<option value="all">${t("全部方法")}</option>` +
      methods.map((m) => `<option value="${esc(m)}">${esc(label(m))}</option>`).join("");

    $("recordFamily").innerHTML =
      `<option value="all">${t("全部类别")}</option>` +
      [...new Set(DATA.curves.map((curve) => curve.family))]
        .sort()
        .map((family) => `<option value="${esc(family)}">${esc(label(family))}</option>`)
        .join("");
  }

  $("language").onchange = (event) => {
    i18n.setLanguage(event.target.value);
    i18n.apply();
    populateOptions();
    app.syncRecordInputs();
    app.render();
    app.notes.refresh();
  };

  $("dataset").onchange = (e) => {
    if (state.view === "battle" && e.target.value === "all") {
      e.target.value = state.dataset;
      app.toast(t("方法对比请选择一个数据集；全量记录可跨库浏览"));
      return;
    }
    state.dataset = e.target.value;
    state.slots.forEach((s) => {
      s.config = null;
      s.curve = null;
    });
    state.paired = false;
    state.zoom = null;
    state.page = 1;
    runtime.hovered = runtime.lockedPoint = null;
    app.render();
  };

  $("target").onchange = (e) => {
    state.target = +e.target.value;
    state.slots.forEach((s) => (s.curve = null));
    state.paired = false;
    state.zoom = null;
    runtime.hovered = runtime.lockedPoint = null;
    app.render();
  };

  $("domain").onchange = (e) => {
    state.domain = e.target.value;
    state.zoom = null;
    runtime.hovered = runtime.lockedPoint = null;
    app.render();
  };

  $("threads")
    .querySelectorAll("button")
    .forEach(
      (b) =>
        (b.onclick = () => {
          state.thread = +b.dataset.thread;
          state.zoom = null;
          runtime.hovered = runtime.lockedPoint = null;
          app.render();
        }),
    );

  $("navigation")
    .querySelectorAll("button")
    .forEach((b) => (b.onclick = () => app.setView(b.dataset.view)));

  document.querySelector(".brand").onclick = (e) => {
    e.preventDefault();
    app.setView("battle");
  };

  $("reset").onclick = app.resetScene;

  $("swapMethods").onclick = () => {
    [state.slots[0], state.slots[1]] = [state.slots[1], state.slots[0]];
    runtime.hovered = runtime.lockedPoint = null;
    app.render();
  };

  $("addMethod").onclick = () => {
    if (state.slots.length >= 4) return;
    const preferred = ["SVS LVQ4×4（共8 bit）", "DiskANN PQ导航 + FP32精排", "HNSWlib FP32"];
    const method = [...preferred, ...methods].find(
      (m) => !state.slots.some((s) => s.method === m) && availability(m),
    );
    if (!method) {
      app.toast(t("该数据集没有更多方法可添加"));
      return;
    }
    state.slots.push({ method, config: null, curve: null });
    app.render();
  };

  $("alignmentNotes").onclick = app.notes.alignmentNotes;

  $("chartTabs")
    .querySelectorAll("button")
    .forEach(
      (b) =>
        (b.onclick = () => {
          state.chart = b.dataset.chart;
          state.zoom = null;
          runtime.hovered = runtime.lockedPoint = null;
          app.charts.scheduleDraw();
        }),
    );

  $("chartReset").onclick = () => {
    state.zoom = null;
    runtime.hovered = runtime.lockedPoint = null;
    app.charts.scheduleDraw();
  };

  $("logAxis").onchange = (e) => {
    state.log = e.target.checked;
    state.zoom = null;
    app.charts.scheduleDraw();
  };

  $("chartExpand").onclick = () => {
    $("chartPanel").classList.toggle("expanded");
    $("chartExpand").textContent = $("chartPanel").classList.contains("expanded") ? "×" : "⛶";
    app.charts.scheduleDraw();
  };

  $("chartExport").onclick = () =>
    $("battleCanvas").toBlob((blob) => {
      if (blob) app.download(`QuIVer-${state.chart}-${state.thread}T.png`, blob);
    });

  $("pointNotes").onclick = () => {
    const r = runtime.lockedPoint || runtime.hovered;
    if (r) app.notes.showRowNotes(r);
  };

  for (const [id, delta] of [
    ["previousPoint", -1],
    ["nextPoint", 1],
  ])
    $(id).onclick = () => {
      if (!runtime.plotRows.length) return;
      const current = runtime.lockedPoint || runtime.hovered,
        index = runtime.plotRows.findIndex(
          (r) => r.curve.id === current?.curve.id && r.threads === current?.threads,
        );
      app.charts.selectPlotPoint(
        runtime.plotRows[
          (Math.max(index, 0) + delta + runtime.plotRows.length) % runtime.plotRows.length
        ],
      );
    };

  $("battleColumns")
    .querySelectorAll("button")
    .forEach(
      (b) =>
        (b.onclick = () => {
          state.columns = b.dataset.columns;
          $("battleColumns")
            .querySelectorAll("button")
            .forEach((x) => x.classList.toggle("active", x === b));
          app.tables.paintTable(
            "battleTable",
            runtime.battleRows.filter(Boolean),
            app.tables.battleKeys[state.columns],
          );
        }),
    );

  $("recordColumns")
    .querySelectorAll("button")
    .forEach(
      (b) =>
        (b.onclick = () => {
          state.recordColumns = b.dataset.columns;
          if (!app.tables.recordKeys[state.recordColumns].includes(state.sort))
            state.sort =
              state.recordColumns === "latency"
                ? "p99"
                : state.recordColumns === "resources"
                  ? "hot"
                  : "qps";
          $("recordColumns")
            .querySelectorAll("button")
            .forEach((x) => x.classList.toggle("active", x === b));
          app.tables.renderRecords();
        }),
    );

  $("exportBattle").onclick = () =>
    app.tables.exportRows(
      runtime.battleRows.filter(Boolean),
      app.tables.battleKeys[state.columns],
      "QuIVer-Battle.csv",
    );

  $("exportRecords").onclick = () =>
    app.tables.exportRows(
      runtime.recordRows,
      app.tables.recordKeys[state.recordColumns],
      "QuIVer-" + t("筛选结果") + ".csv",
    );

  for (const id of ["recordMethod", "recordFamily", "recordThread"])
    $(id).onchange = (e) => {
      state[id] = e.target.value;
      state.page = 1;
      app.tables.renderRecords();
    };

  $("recordRecall").oninput = (e) => {
    state.recordRecall = Math.max(0, +e.target.value || 0);
    state.page = 1;
    app.tables.renderRecords();
  };

  let recordTimer;

  $("recordSearch").oninput = (e) => {
    clearTimeout(recordTimer);
    recordTimer = setTimeout(() => {
      state.recordSearch = e.target.value.trim();
      state.page = 1;
      app.tables.renderRecords();
    }, 120);
  };

  $("clearRecords").onclick = () => {
    state.recordSearch = "";
    state.recordMethod = state.view === "quiver" ? "QuIVer" : "all";
    state.recordFamily = "all";
    state.recordThread = "all";
    state.recordRecall = 0;
    state.page = 1;
    app.syncRecordInputs();
    app.tables.renderRecords();
  };

  $("pageSize").onchange = (e) => {
    state.pageSize = e.target.value;
    state.page = 1;
    app.tables.renderRecords();
  };

  $("previousPage").onclick = () => {
    state.page--;
    app.tables.renderRecords();
  };

  $("nextPage").onclick = () => {
    state.page++;
    app.tables.renderRecords();
  };

  for (const id of [
    "globalNotes",
    "footerNotes",
    "snapshotButton",
    "chartHelp",
    "availabilityNotes",
  ])
    $(id).onclick = app.notes.globalNotes;

  $("closeDrawer").onclick = app.notes.closeDrawer;

  $("drawerBackdrop").onclick = app.notes.closeDrawer;

  $("themeButton").onclick = () => {
    document.body.classList.toggle("dark");
    try {
      localStorage.setItem(
        "quiver-battle-theme",
        document.body.classList.contains("dark") ? "dark" : "light",
      );
    } catch {}
    app.render();
  };

  try {
    if (localStorage.getItem("quiver-battle-theme") === "dark") document.body.classList.add("dark");
  } catch {}

  $("shareView").onclick = async () => {
    const current = {
      view: "battle",
      lang: i18n.getLanguage(),
      dataset: state.dataset,
      thread: state.thread,
      target: state.target,
      domain: state.domain,
      chart: state.chart,
      slots: state.slots,
      paired: state.paired,
    };
    const url = location.href.split("#")[0] + "#" + encodeURIComponent(JSON.stringify(current));
    try {
      await navigator.clipboard.writeText(url);
      app.toast(t("已复制包含方法、参数和线程的比较视图"));
    } catch {
      history.replaceState(null, "", url);
      app.toast(t("当前比较已保存到地址栏，可复制地址"));
    }
  };

  try {
    const saved = JSON.parse(decodeURIComponent(location.hash.slice(1)));
    if (saved.lang) i18n.setLanguage(saved.lang);
    if (ds.has(saved.dataset)) {
      state.dataset = saved.dataset;
      if ([1, 8, 16, 32].includes(saved.thread)) state.thread = saved.thread;
      if ([0.9, 0.95, 0.98].includes(saved.target)) state.target = saved.target;
      if (["request", "batch"].includes(saved.domain)) state.domain = saved.domain;
      if (["recall", "memory", "scaling", "latency", "build", "footprint"].includes(saved.chart))
        state.chart = saved.chart;
      state.paired = saved.paired === true;
      if (
        Array.isArray(saved.slots) &&
        saved.slots.length >= 2 &&
        saved.slots.length <= 4 &&
        saved.slots.every((s) => s && methods.includes(s.method)) &&
        new Set(saved.slots.map((s) => s.method)).size === saved.slots.length
      )
        state.slots = saved.slots.map((slot) => ({
          method: slot.method,
          config: cfg.get(slot.config)?.method === slot.method ? slot.config : null,
          curve: curveById.get(slot.curve)?.method === slot.method ? slot.curve : null,
        }));
    }
  } catch {}

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      if (!$("notesDrawer").hidden) app.notes.closeDrawer();
      else {
        $("chartPanel").classList.remove("expanded");
        $("chartExpand").textContent = "⛶";
        state.zoom = null;
        app.charts.scheduleDraw();
      }
    }
    if (e.key === "Tab" && !$("notesDrawer").hidden) {
      const elements = [
        ...$("notesDrawer").querySelectorAll(
          "button, summary, a[href], input, select, [tabindex='0']",
        ),
      ];
      const first = elements[0],
        last = elements[elements.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  });

  new ResizeObserver(app.charts.scheduleDraw).observe($("battleCanvas").parentElement);

  i18n.apply();
  $("language").value = i18n.getLanguage();
  populateOptions();
  app.syncRecordInputs();

  app.render();
})();
