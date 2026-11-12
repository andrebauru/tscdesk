const { test } = require("node:test");
const assert = require("node:assert/strict");
const { JSDOM } = require("jsdom");
const fs = require("node:fs");
const path = require("node:path");

const ui = path.join(__dirname, "../../resources/ui");
const sources = ["i18n/en.js", "i18n/pt-BR.js", "i18n/es.js", "i18n.js"]
  .map(file => fs.readFileSync(path.join(ui, file), "utf8"));
const wait = () => new Promise(resolve => setTimeout(resolve, 0));

function fixture(language = "en-US", saved = null) {
  const dom = new JSDOM("<!doctype html><html><body><main id='app'></main></body></html>", {
    url: "https://app.localhost/", runScripts: "outside-only", pretendToBeVisual: true,
  });
  Object.defineProperty(dom.window.navigator, "language", { value: language, configurable: true });
  Object.defineProperty(dom.window.navigator, "languages", { value: [language], configurable: true });
  if (saved) dom.window.localStorage.setItem("tscide.language", saved);
  for (const source of sources) dom.window.eval(source);
  return dom;
}

test("catalogs expose the same semantic keys in English, Portuguese and Spanish", () => {
  const dom = fixture();
  const catalogs = dom.window.tscI18n.catalogs;
  const expected = Object.keys(catalogs.en).sort();
  assert.deepEqual(Object.keys(catalogs["pt-BR"]).sort(), expected);
  assert.deepEqual(Object.keys(catalogs.es).sort(), expected);
  assert.ok(expected.every(key => key.includes(".")));
  dom.window.close();
});

test("first run detects supported languages and falls back to English", () => {
  for (const [browserLanguage, expected] of [["pt-PT", "pt-BR"], ["es-MX", "es"], ["fr-FR", "en"]]) {
    const dom = fixture(browserLanguage);
    assert.equal(dom.window.tscI18n.getLocale(), expected);
    assert.equal(dom.window.document.documentElement.lang, expected);
    dom.window.close();
  }
});

test("saved choice wins, persists and updates translated DOM immediately", async () => {
  const dom = fixture("en-US", "pt-BR");
  const { document, tscI18n, localStorage } = dom.window;
  const title = document.createElement("h1");
  title.textContent = "Settings";
  const input = document.createElement("input");
  input.setAttribute("placeholder", "Search");
  document.getElementById("app").append(title, input);
  await wait();
  assert.equal(title.textContent, "Configurações");
  assert.equal(input.placeholder, "Pesquisar");
  tscI18n.setLocale("es");
  title.textContent = tscI18n.t("common.settings");
  assert.equal(title.textContent, "Configuración");
  assert.equal(document.documentElement.lang, "es");
  assert.equal(localStorage.getItem("tscide.language"), "es");
  dom.window.close();
});

test("missing keys fall back to English and user/model content is not translated", async () => {
  const dom = fixture("es");
  const { document, tscI18n } = dom.window;
  delete tscI18n.catalogs.es["common.close"];
  assert.equal(tscI18n.t("common.close"), "Close");
  const message = document.createElement("article");
  message.className = "message-body";
  message.textContent = "Settings";
  document.body.append(message);
  await wait();
  assert.equal(message.textContent, "Settings");
  dom.window.close();
});

test("visible UI sources use TsCIDE and do not contain legacy product names", () => {
  const files = ["index.html", "app.js", "capability-workflows.js", "schedule-workflows.js", "workspace-workflows.js"];
  for (const file of files) {
    const source = fs.readFileSync(path.join(ui, file), "utf8");
    assert.doesNotMatch(source, /Quixdex|QuixiDesk/g, file);
  }
});
