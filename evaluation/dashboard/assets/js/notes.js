"use strict";

// 翻译展示层的备注，权威记录及其证据身份保持独立。
(() => {
  const app = window.QuiverDashboard;
  const { DATA, $, esc, ds, state, runtime, fmt, pct, t, label, i18n } = app;

  function noteBlock(title, body) {
    if (body == null || body === "") return "";
    const content = i18n.localize(body);
    const expanded = ["任务与完整查询", "构建及查询参数", "当前快照", "当前比较场景"].includes(
      title,
    );
    return `<details class="note-block" ${expanded ? "open" : ""}><summary>${esc(t(title))}</summary><p>${esc(typeof content === "object" ? JSON.stringify(content, null, 2) : content)}</p></details>`;
  }

  function openDrawer(title, html) {
    runtime.drawerReturn = document.activeElement;
    $("drawerTitle").textContent = title;
    $("drawerBody").innerHTML = html;
    $("notesDrawer").hidden = false;
    $("drawerBody").scrollTop = 0;
    $("drawerBackdrop").hidden = false;
    $("closeDrawer").focus();
  }

  function closeDrawer() {
    $("notesDrawer").hidden = true;
    $("drawerBackdrop").hidden = true;
    runtime.notesContext = null;
    runtime.drawerReturn?.focus();
  }

  function showRowNotes(row) {
    runtime.notesContext = { type: "row", row };
    const dataset = ds.get(row.dataset);
    const point = row.point
      ? {
          ...row.point,
          samples: state.domain === "batch" ? row.point.batchSamples : row.point.samples,
        }
      : null;
    const evidence = point?.evidence || row.evidence;
    let html = "";
    if (dataset.metric === "euclidean")
      html += `<div class="note-alert">${t("SIFT/GIST是论文明确的预期不适用边界对照。本次保留原始幅值的原生L2结果不与旧稿归一化余弦结果混比，不用通用95%目标评价QuIVer贡献。")}</div>`;
    if (point?.route === "BQ")
      html += `<div class="note-alert">${t("此位置实际为旧版BQ回退，不能标为纯SM-L2。旧请求budget64大于ef32/48，修正结果完整核验后才会增补当前曲线。")}</div>`;
    if (!point)
      html += `<div class="note-alert">${t("当前线程未测该参数位置。没有使用其他线程的QPS或尾延迟补填。")}</div>`;
    html += noteBlock(
      "任务与完整查询",
      t(
        "{dataset} · {size} × {dim}\n度量：{metric}\n线程：{threads}\n评估查询：{queries}\n实际导航：{route}\n精排实际预算：{budget}",
        {
          dataset: dataset.name,
          size: fmt(dataset.n),
          dim: dataset.dim,
          metric: dataset.metric === "euclidean" ? t("原生平方L2") : t("余弦"),
          threads: row.threads || state.thread,
          queries: fmt(point?.queries),
          route: label(point?.route || "未记录"),
          budget: point?.actualBudget ?? t("见冻结协议"),
        },
      ),
    );
    html += noteBlock(
      "构建及查询参数",
      t("构建：{build}\n查询：{query}", {
        build: label(row.buildConfig),
        query: label(row.params),
      }),
    );
    html += noteBlock(
      "冻结预选记录",
      row.curve.targets
        .map(
          (target) =>
            (target.target == null ? t("固定预算") : pct(target.target)) +
            " · " +
            t(target.note || "冻结调参选择"),
        )
        .join("\n"),
    );
    if (state.paired || state.slots.some((slot) => slot.curve === row.curve.id))
      html += noteBlock(
        "浏览选点",
        "当前浏览可能手动或按相近实测召回配对，不改写冻结调参、原始结果或论文预选档。没有插值制造测量点。",
      );
    if (point?.samples?.length) {
      const maximum = Math.max(...point.samples);
      html +=
        '<div class="rounds">' +
        point.samples
          .map(
            (value, index) =>
              `<div><small>${fmt(value)}</small><i style="height:${(value / maximum) * 55}px"></i><small>${t("轮{round}", { round: index + 1 })}</small></div>`,
          )
          .join("") +
        "</div>";
    }
    for (const [key, value] of Object.entries({ ...row.notes, ...point?.notes }))
      html += noteBlock(key, value);
    if (point?.p95ci?.length && point?.p99ci?.length)
      html += noteBlock(
        "请求尾分位数95%查询簇区间",
        `P95: [${point.p95ci.map((value) => fmt(value, 3)).join(", ")}] ms\nP99: [${point.p99ci.map((value) => fmt(value, 3)).join(", ")}] ms`,
      );
    if (!row.exact)
      html += noteBlock(
        "热组件下界",
        t(
          "当前 {memory} GiB 为已确认持久组件小计，不包含全部管理对象、运行时、分配器及工作区。逻辑冷表示精排访问角色，不表示实际不驻留。",
          { memory: fmt(row.hot, 6) },
        ),
      );
    const componentNames = {
      nav: "FP32导航",
      codes: "量化存储",
      graph: "图组件",
      factors: "因子/布局",
    };
    html += noteBlock(
      "持久组件 / GiB",
      Object.entries(row.components)
        .map(([key, value]) => t(componentNames[key] || key) + ": " + fmt(value, 6))
        .join("\n"),
    );
    html += noteBlock(
      "QPS与计时类别",
      t(
        "{domain}；当前为{mode}。库API类别相同不保证包装时钟完全相同，应结合本条吞吐、加载及构建备注解读。",
        {
          domain: label(row.curve.domain),
          mode: t(state.domain === "batch" ? "FAISS官方批量" : "请求或库接口"),
        },
      ),
    );
    html += noteBlock(
      "证据身份 / SHA-256",
      Object.entries(evidence || {})
        .map(([key, value]) => t(key) + "\n" + value)
        .join("\n\n"),
    );
    openDrawer(dataset.name + " / " + label(row.method), html);
  }

  function alignmentNotes() {
    runtime.notesContext = { type: "alignment" };
    const [a, b] = runtime.battleRows;
    let html = noteBlock(
      "当前比较场景",
      `${ds.get(state.dataset)?.name} · ${state.thread}T · ${t(state.domain === "batch" ? "FAISS官方批量" : "请求 / 库接口")}\nA: ${label(a?.method || "无数据")}\nB: ${label(b?.method || "无数据")}`,
    );
    html += noteBlock(
      "实测召回",
      t(
        "A：{a}\nB：{b}\n差：{gap}\n页面以0.5个百分点作为接近提示；这不是严格等召回，也不会将未对齐点包装为matched-recall胜负。",
        {
          a: pct(a?.recall),
          b: pct(b?.recall),
          gap:
            a?.recall != null && b?.recall != null
              ? fmt(Math.abs(a.recall - b.recall) * 100, 4) + " pp"
              : t("未形成完整配对"),
        },
      ),
    );
    html += noteBlock(
      "当前点与近似浏览",
      "默认来自冻结调参的预选档；手动选点或近似同召回浏览只改变当前页面，不改实验参数。自动浏览在双方当前构建配置已测点中寻找达到所选召回阈值、接近该阈值且召回相近的组合，不插值，不推测未测性能。",
    );
    html += noteBlock(
      "吞吐边界",
      t(
        "A：{a}\nB：{b}\n不同库接口不能视为完全相同的服务边界。FAISS官方批量已另设口径，不与其逐请求QPS替换。",
        { a: label(a?.curve.domain || "—"), b: label(b?.curve.domain || "—") },
      ),
    );
    html += noteBlock(
      "内存与构建比较",
      "四栏仅比较当前两个实测点，不给系统打综合分。优势倍数均显示领先方相对落后方：吞吐为高值/低值，内存、构建、延迟为高成本/低成本。优势条越长表示越强：吞吐按QPS分配，其余按成本倒数分配；占比不是原始资源用量。吞吐领先幅度以落后方为基准计算增幅，其余领先幅度表示相对落后方减少的成本。热组件为已核组件小计，多数为下界；构建计时包装范围可能不同，须核各自备注。",
    );
    openDrawer(t("配对与可比性备注"), html);
  }

  function globalNotes() {
    runtime.notesContext = { type: "global" };
    const all = DATA.curves.flatMap((curve) =>
      Object.values(curve.points).map((point) => ({ curve, point })),
    );
    const own = all.filter((record) => record.curve.method === "QuIVer");
    let html = noteBlock(
      "当前快照",
      t(
        "{points} 个观测、{curves} 组参数、{configs} 个构建配置。外部基线 {external} 个位置覆盖三个核心库；QuIVer {own} 个位置覆盖 {datasets} 库。\n生成时间：{date}。页面使用离线快照。",
        {
          points: fmt(all.length),
          curves: fmt(DATA.curves.length),
          configs: fmt(DATA.configs.length),
          external: fmt(all.length - own.length),
          own: fmt(own.length),
          datasets: DATA.datasets.length,
          date: DATA.meta.generatedAt || DATA.meta.updated,
        },
      ),
    );
    html += noteBlock(
      "使用顺序",
      "选择数据集、线程和吞吐口径，然后选择A/B方法、构建配置与查询点。查看实测召回差后，切换图表或展开记录备注。可添加C/D，最多四个方法同时绘图。",
    );
    html += noteBlock(
      "图表操作",
      "实心点为实测位置，带外圈为当前比较点；三角形表示旧BQ回退。点间连线不创造额外样本。曲线按构建配置和实际导航路径分组，BQ与SM-L2不跨路径连线。悬停查看点，点击选为当前比较点；滚轮缩放、拖动平移、双击或还原视域复位；点击图例隐藏方法。线程图虚线为理想线性扩展。",
    );
    html += noteBlock(
      "缺项与配对",
      "未测线程、缺少原生尾数组或没有逐点驻留均保留空白。预选档不代表所有数据分布都承诺达到该召回；SIFT/GIST是预期不适用边界。相同API类别与相同查询数只是检查提示，不是完整公平性证明。",
    );
    html += DATA.notes.map((note) => noteBlock(note.title, note.body)).join("");
    html += noteBlock(
      "源文件SHA-256",
      Object.entries(DATA.meta.sourceHashes)
        .map(([key, value]) => t(key) + "\n" + value)
        .join("\n\n"),
    );
    openDrawer(t("备注、数据来源与操作指引"), html);
  }

  function refresh() {
    if ($("notesDrawer").hidden) return;
    const context = runtime.notesContext;
    if (context?.type === "row") showRowNotes(context.row);
    else if (context?.type === "alignment") alignmentNotes();
    else globalNotes();
  }

  Object.assign(app.notes, {
    noteBlock,
    openDrawer,
    closeDrawer,
    showRowNotes,
    alignmentNotes,
    globalNotes,
    refresh,
  });
})();
