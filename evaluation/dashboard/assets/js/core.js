"use strict";

// 数据入口、共享状态和格式化函数供各职责模块复用。
(() => {
  const DATA = window.QuiverBenchmarkData;
  const i18n = window.QuiverI18n;
  const { t, label } = i18n;
  const $ = (id) => document.getElementById(id);
  const esc = (value) =>
    String(value ?? "").replace(
      /[&<>"']/g,
      (character) =>
        ({
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        })[character],
    );
  const ds = new Map(DATA.datasets.map((record) => [record.id, record]));
  const cfg = new Map(DATA.configs.map((record) => [record.id, record]));
  const curveById = new Map(DATA.curves.map((record) => [record.id, record]));
  const methods = [...new Set(DATA.curves.map((record) => record.method))].sort((a, b) =>
    a === "QuIVer" ? -1 : b === "QuIVer" ? 1 : a.localeCompare(b),
  );
  const state = {
    view: "battle",
    dataset: "D03",
    thread: 1,
    target: 0.95,
    domain: "request",
    chart: "recall",
    log: true,
    slots: [
      { method: "QuIVer", config: null, curve: null },
      { method: "Lucene BBQ + FP32", config: null, curve: null },
    ],
    columns: "core",
    recordColumns: "performance",
    recordSearch: "",
    recordMethod: "all",
    recordFamily: "all",
    recordThread: "all",
    recordRecall: 0,
    sort: "qps",
    direction: -1,
    page: 1,
    pageSize: 25,
    hidden: new Set(),
    paired: false,
    zoom: null,
  };
  const runtime = {
    battleRows: [],
    recordRows: [],
    plotRows: [],
    hits: [],
    hovered: null,
    lockedPoint: null,
    drawPending: false,
    drawerReturn: null,
    chartBox: null,
    drag: null,
    dragMoved: false,
  };
  const fmt = (value, digits = 0) =>
    value == null || !Number.isFinite(+value)
      ? "—"
      : Number(value).toLocaleString(i18n.getLocale(), {
          minimumFractionDigits: digits,
          maximumFractionDigits: digits,
        });
  const pct = (value) => (value == null ? "—" : fmt(value * 100, 2) + "%");
  const compact = (value) =>
    value == null
      ? "—"
      : value >= 1e6
        ? fmt(value / 1e6, 1) + "M"
        : value >= 1e3
          ? fmt(value / 1e3, 1) + "K"
          : fmt(value, Math.abs(value) < 10 ? 2 : 0);
  const qps = (point) =>
    !point ? null : state.domain === "batch" ? point.batch : point.eligible ? point.qps : null;
  const slotInk = (index) =>
    document.body.classList.contains("dark")
      ? ["#62d4ba", "#a1b1ff", "#edc27a", "#d4a5ea"][index]
      : ["#087f75", "#665bd8", "#b77b18", "#9e4aab"][index];
  const methodColor = (method) => {
    const index = state.slots.findIndex((slot) => slot.method === method);
    return index >= 0 ? slotInk(index) : method === "QuIVer" ? slotInk(0) : "#8e9ba9";
  };
  const availability = (method) =>
    DATA.curves.some((curve) => curve.dataset === state.dataset && curve.method === method);
  const shortMethod = (method) => (method === "Lucene BBQ + FP32" ? "BBQ" : label(method));
  window.QuiverDashboard = {
    DATA,
    i18n,
    t,
    label,
    $,
    esc,
    ds,
    cfg,
    curveById,
    methods,
    state,
    runtime,
    fmt,
    pct,
    compact,
    qps,
    slotInk,
    methodColor,
    availability,
    letters: ["A", "B", "C", "D"],
    shortMethod,
    model: {},
    battle: {},
    tables: {},
    coverage: {},
    charts: {},
    notes: {},
  };
})();
