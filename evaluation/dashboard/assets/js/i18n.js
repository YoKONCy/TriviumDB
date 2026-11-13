"use strict";

// 界面翻译与原始记录分离，方法身份和实验数值始终使用权威数据。
(() => {
  const catalogs = window.QuiverLocaleData;
  const supported = ["zh-CN", "en"];
  let language =
    typeof navigator !== "undefined" && !navigator.language.toLowerCase().startsWith("zh")
      ? "en"
      : "zh-CN";
  try {
    const saved = localStorage.getItem("quiver-dashboard-language");
    if (supported.includes(saved)) language = saved;
  } catch {}

  function t(source, values) {
    const original = String(source ?? "");
    const entry = language === "en" ? (catalogs.en[original] ?? original) : original;
    const category =
      typeof entry === "object"
        ? new Intl.PluralRules("en").select(Number(String(values?.count ?? 0).replaceAll(",", "")))
        : null;
    const translated = category ? (entry[category] ?? entry.other) : entry;
    if (!values) return translated;
    return translated.replace(/\{(\w+)\}/g, (token, key) =>
      Object.hasOwn(values, key) ? String(values[key]) : token,
    );
  }

  function setLanguage(next) {
    if (!supported.includes(next)) return;
    language = next;
    try {
      localStorage.setItem("quiver-dashboard-language", language);
    } catch {}
  }

  function apply(root = document) {
    document.documentElement.lang = language;
    for (const element of root.querySelectorAll("[data-i18n]")) {
      element.textContent = t(element.dataset.i18n);
    }
    for (const attribute of ["title", "placeholder", "aria-label"]) {
      for (const element of root.querySelectorAll(`[data-i18n-${attribute}]`)) {
        element.setAttribute(attribute, t(element.getAttribute(`data-i18n-${attribute}`)));
      }
    }
  }

  function localize(value) {
    if (typeof value === "string") return t(value);
    if (Array.isArray(value)) return value.map(localize);
    if (value && typeof value === "object") {
      return Object.fromEntries(
        Object.entries(value).map(([key, item]) => [t(key), localize(item)]),
      );
    }
    if (typeof value === "boolean") return t(value ? "是" : "否");
    return value;
  }

  window.QuiverI18n = {
    t,
    label: t,
    localize,
    setLanguage,
    apply,
    searchText: (source) => String(source ?? "") + " " + (catalogs.en[source] ?? ""),
    getLanguage: () => language,
    getLocale: () => (language === "en" ? "en-US" : "zh-CN"),
  };
})();
