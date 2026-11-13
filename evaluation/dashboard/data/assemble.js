"use strict";

// 按原始顺序装配完整快照，恢复各数据集独立存放的备注与证据。
(() => {
  const parts = window.QuiverBenchmarkParts;
  function restore(value, pool) {
    if (Array.isArray(value)) return value.map((item) => restore(item, pool));
    if (!value || typeof value !== "object") return value;
    if (Object.keys(value).length === 1 && "$annotation" in value) {
      const index = value.$annotation;
      if (!Number.isInteger(index) || index < 0 || index >= pool.length) {
        throw new Error("数据注释引用无效");
      }
      return restore(pool[index], []);
    }
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, restore(item, pool)]),
    );
  }
  const configs = new Map();
  const curves = new Map();
  for (const dataset of parts.catalog.datasets) {
    const pool = parts.annotations[dataset.id];
    for (const item of restore(parts.configurations[dataset.id], pool)) {
      configs.set(item.id, item);
      for (const curve of restore(parts.observations[item.id], pool)) curves.set(curve.id, curve);
    }
  }
  window.QuiverBenchmarkData = {
    meta: parts.catalog.meta,
    datasets: parts.catalog.datasets,
    configs: parts.catalog.configOrder.map((id) => configs.get(id)),
    curves: parts.catalog.curveOrder.map((id) => curves.get(id)),
    historicalBQ: parts.historicalBQ,
    notes: parts.notes,
  };
  delete window.QuiverBenchmarkParts;
})();
