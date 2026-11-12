(() => {
  "use strict";
  const STORAGE_KEY = "tscide.language";
  const supported = ["en", "pt-BR", "es"];
  const catalogs = window.__TSCIDE_TRANSLATIONS__ || {};
  const normalize = (value) => {
    const code = String(value || "").toLowerCase();
    if (code.startsWith("pt")) return "pt-BR";
    if (code.startsWith("es")) return "es";
    return "en";
  };
  const detected = normalize(navigator.languages?.[0] || navigator.language);
  let locale = normalize(localStorage.getItem(STORAGE_KEY) || detected);
  const englishIndex = new Map(Object.entries(catalogs.en || {}).map(([key, value]) => [value, key]));

  // Retorna uma traducao com fallback deterministico para ingles.
  function t(key, values = {}) {
    let text = catalogs[locale]?.[key] ?? catalogs.en?.[key];
    if (text == null) {
      console.warn(`[i18n] Missing translation key: ${key}`);
      return key;
    }
    return String(text).replace(/\{(\w+)\}/g, (_, name) => values[name] ?? `{${name}}`);
  }
  const ignored = (node) => node.parentElement?.closest(".message-body, .thinking-body, pre, code, textarea, [data-i18n-ignore]");
  function translateText(node) {
    if (node.nodeType !== Node.TEXT_NODE || ignored(node)) return;
    const raw = node.nodeValue;
    const trimmed = raw.trim();
    const key = englishIndex.get(trimmed);
    if (key) node.nodeValue = raw.replace(trimmed, t(key));
  }
  function translateElement(element) {
    if (!(element instanceof Element) || element.closest(".message-body, pre, code, [data-i18n-ignore]")) return;
    for (const attr of ["aria-label", "title", "placeholder"]) {
      const value = element.getAttribute(attr);
      const key = englishIndex.get(value);
      if (key) element.setAttribute(attr, t(key));
    }
    for (const child of element.childNodes) {
      if (child.nodeType === Node.TEXT_NODE) translateText(child);
      else if (child.nodeType === Node.ELEMENT_NODE) translateElement(child);
    }
  }
  function apply(root = document.body) {
    document.documentElement.lang = locale;
    if (root) translateElement(root);
  }
  function setLocale(next) {
    locale = supported.includes(next) ? next : normalize(next);
    localStorage.setItem(STORAGE_KEY, locale);
    document.documentElement.lang = locale;
    window.dispatchEvent(new CustomEvent("tscide:language-changed", { detail: { locale } }));
    apply();
  }
  new MutationObserver((records) => {
    for (const record of records) for (const node of record.addedNodes) {
      if (node.nodeType === Node.TEXT_NODE) translateText(node);
      else if (node.nodeType === Node.ELEMENT_NODE) translateElement(node);
    }
  }).observe(document.documentElement, { childList: true, subtree: true });
  window.tscI18n = Object.freeze({ t, apply, setLocale, getLocale: () => locale, supported, catalogs, storageKey: STORAGE_KEY });
  apply(document.documentElement);
})();
