// Copyright (c) 2026 QuixiAI. MIT license.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { JSDOM } = require("jsdom");
const fs = require("node:fs");
const path = require("node:path");

const explorerSource = fs.readFileSync(
  path.join(__dirname, "../../resources/ui/explorer.js"),
  "utf8",
);
const appSource = fs.readFileSync(
  path.join(__dirname, "../../resources/ui/app.js"),
  "utf8",
);
const indexHtml = fs.readFileSync(
  path.join(__dirname, "../../resources/ui/index.html"),
  "utf8",
);
const appCss = fs.readFileSync(
  path.join(__dirname, "../../resources/ui/app.css"),
  "utf8",
);

const wait = () => new Promise((resolve) => setTimeout(resolve, 20));
const settle = async () => {
  for (let index = 0; index < 6; index += 1) await wait();
};

function helpers(window) {
  const errors = [];
  const dialogs = [];
  const toasts = [];
  const el = (tag, attrs = {}, ...children) => {
    const node = window.document.createElement(tag);
    for (const [key, value] of Object.entries(attrs)) {
      if (value === undefined || value === null || value === false) continue;
      if (key === "class") node.className = value;
      else if (key === "style") node.setAttribute("style", value);
      else if (key.startsWith("on"))
        node.addEventListener(key.slice(2).toLowerCase(), value);
      else if (key === "checked" || key === "hidden")
        node[key] = Boolean(value);
      else node.setAttribute(key, value === true ? "" : value);
    }
    for (const child of children.flat(Infinity)) {
      if (child === null || child === undefined || child === false) continue;
      node.append(
        child instanceof window.Node
          ? child
          : window.document.createTextNode(String(child)),
      );
    }
    return node;
  };
  const icon = (name) => {
    const node = el("span", { class: "test-icon" });
    node.dataset.icon = name;
    return node;
  };
  const button = (label, fn, cls = "", name) => {
    const node = el("button", { type: "button", class: cls }, name ? icon(name) : null, label);
    node.addEventListener("click", async (event) => {
      event.stopPropagation();
      try {
        await fn(event);
      } catch (error) {
        errors.push(error.message);
      }
    });
    return node;
  };
  const ib = (label, name, fn) => {
    const node = button("", fn, "icon quiet", name);
    node.setAttribute("aria-label", label);
    return node;
  };
  const dialog = (title, ...content) => {
    dialogs.push({ title, content });
    const box = el("div", { class: "test-dialog" }, el("h3", {}, title), ...content);
    window.document.body.append(box);
  };
  return {
    errors,
    dialogs,
    toasts,
    el,
    icon,
    button,
    ib,
    dialog,
    toast: (text, error) => toasts.push({ text, error }),
  };
}

function explorerState(overrides = {}) {
  return {
    version: 1,
    sessionId: "session-one",
    preferences: {
      showHidden: false,
      showIgnored: false,
      extraExcludes: [],
      maxFileBytes: 262144,
      maxTotalChars: 200000,
    },
    sources: [
      {
        id: "source-finance",
        kind: "folder",
        path: "/home/user/financeiro",
        label: "PROJETO FINANCEIRO",
        projectKind: "php",
        selected: true,
        selectedPaths: [],
        deselectedPaths: [],
        expanded: [],
        missing: false,
      },
    ],
    selectedFiles: 2,
    selectedBytes: 1200,
    selectionCapped: false,
    ...overrides,
  };
}

const treeEntries = {
  "": [
    {
      name: "assets",
      relativePath: "assets",
      isDir: true,
      size: 0,
      checked: true,
      partial: false,
    },
    {
      name: "index.php",
      relativePath: "index.php",
      isDir: false,
      size: 100,
      checked: true,
      partial: false,
    },
    {
      name: "README.md",
      relativePath: "README.md",
      isDir: false,
      size: 200,
      checked: false,
      partial: false,
    },
    {
      name: "ação & ção.txt",
      relativePath: "ação & ção.txt",
      isDir: false,
      size: 12,
      checked: true,
      partial: false,
    },
  ],
  assets: [
    {
      name: "style.css",
      relativePath: "assets/style.css",
      isDir: false,
      size: 50,
      checked: true,
      partial: false,
    },
  ],
};

function fixture({ secondSessionEmpty = true } = {}) {
  const dom = new JSDOM(
    '<!doctype html><html><body><div id="app"></div><div id="toasts"></div></body></html>',
    { url: "https://app.localhost/chat", runScripts: "outside-only", pretendToBeVisual: true },
  );
  const window = dom.window;
  const calls = [];
  const states = new Map([
    ["session-one", explorerState()],
    [
      "session-two",
      secondSessionEmpty
        ? explorerState({ sessionId: "session-two", sources: [], selectedFiles: 0, selectedBytes: 0 })
        : explorerState({ sessionId: "session-two" }),
    ],
  ]);
  let preview = {
    selectedFiles: 2,
    estimatedBytes: 1200,
    estimatedChars: 1400,
    budgetChars: 200000,
    exceedsBudget: false,
    capped: false,
    missingSources: [],
    sources: 1,
  };
  const rpc = async (service, member, args = []) => {
    calls.push({ service, member, args });
    const body = args[0] || {};
    const sessionId = body.sessionId;
    const state = states.get(sessionId);
    if (service === "FileSystem" && member === "browseFolder") return "/home/user/documentos";
    if (service === "FileSystem" && member === "browseFiles")
      return ["/home/user/configuracao.json", "/home/user/requisitos.txt"];
    if (service !== "Explorer") return null;
    if (member === "getState") return structuredClone(state);
    if (member === "listDirectory") {
      return { entries: structuredClone(treeEntries[body.relativePath || ""] || []), truncated: false };
    }
    if (member === "contextPreview") return structuredClone(preview);
    if (member === "addSource") {
      state.sources.push({
        id: "source-added",
        kind: "folder",
        path: body.path,
        label: "documentos",
        projectKind: null,
        selected: false,
        selectedPaths: [],
        deselectedPaths: [],
        expanded: [],
        missing: false,
      });
      return structuredClone(state);
    }
    if (member === "addSources") {
      for (const file of body.paths) {
        state.sources.push({
          id: "file-" + file,
          kind: "file",
          path: file,
          label: file.split("/").pop(),
          projectKind: null,
          selected: true,
          selectedPaths: [],
          deselectedPaths: [],
          expanded: [],
          missing: false,
        });
      }
      return structuredClone(state);
    }
    if (member === "setSourceSelected") {
      const source = state.sources.find((item) => item.id === body.sourceId);
      source.selected = body.selected;
      source.selectedPaths = [];
      source.deselectedPaths = [];
      state.selectedFiles = body.selected ? 3 : 0;
      return structuredClone(state);
    }
    if (member === "setFileSelected") {
      const source = state.sources.find((item) => item.id === body.sourceId);
      if (source.selected) {
        source.deselectedPaths = body.selected
          ? source.deselectedPaths.filter((item) => item !== body.relativePath)
          : [...source.deselectedPaths, body.relativePath];
      } else {
        source.selectedPaths = body.selected
          ? [...source.selectedPaths, body.relativePath]
          : source.selectedPaths.filter((item) => item !== body.relativePath);
      }
      state.selectedFiles = body.selected ? 3 : 2;
      return structuredClone(state);
    }
    if (member === "setDirectorySelected") {
      state.selectedFiles = body.selected ? 3 : 1;
      return structuredClone(state);
    }
    if (member === "setPreferences") {
      Object.assign(state.preferences, body);
      delete state.preferences.sessionId;
      return structuredClone(state);
    }
    if (member === "selectAll") {
      for (const source of state.sources) {
        source.selected = body.selected;
        source.selectedPaths = [];
        source.deselectedPaths = [];
      }
      state.selectedFiles = body.selected ? 3 : 0;
      return structuredClone(state);
    }
    if (member === "removeSource") {
      state.sources = state.sources.filter((item) => item.id !== body.sourceId);
      return structuredClone(state);
    }
    if (member === "refresh") {
      return structuredClone(state);
    }
    if (member === "previewFile") {
      return {
        relativePath: body.relativePath,
        content: "<?php echo 'olá';",
        encoding: "utf-8",
        binary: false,
        truncated: false,
        bytesRead: 18,
        error: null,
      };
    }
    return { error: "Unknown Explorer member " + member };
  };
  window.quixi = { rpc, api: async () => null, copy: async () => {}, origin: "https://app.localhost" };
  window.eval(explorerSource);
  const h = helpers(window);
  const host = window.document.createElement("div");
  window.document.body.append(host);
  const panel = window.TsExplorerPanel.create({
    el: h.el,
    button: h.button,
    ib: h.ib,
    icon: h.icon,
    toast: h.toast,
    rpc,
    dialog: h.dialog,
  });
  return { window, host, panel, calls, states, h, setPreview: (value) => (preview = value) };
}

const byText = (window, selector, text) =>
  Array.from(window.document.querySelectorAll(selector)).find(
    (node) => node.textContent.trim() === text,
  );

test("renders EXPLORER sources, selection count and lazy tree rows", async (t) => {
  const f = fixture();
  t.after(() => f.panel.dispose());
  await f.panel.render(f.host, { sessionId: "session-one", title: "Financeiro" }, { projectName: "Project A" });
  await settle();
  const text = f.host.textContent;
  assert.match(text, /EXPLORER/);
  assert.match(text, /PROJETO FINANCEIRO/);
  assert.match(text, /2 files selected/);
  assert.match(text, /Context: Project A/);
  assert.ok(byText(f.window, ".explorer-name", "index.php"));
  assert.ok(byText(f.window, ".explorer-name", "ação & ção.txt"));

  const assets = byText(f.window, ".explorer-name", "assets");
  assets.click();
  await settle();
  assert.ok(byText(f.window, ".explorer-name", "style.css"));
  const expand = f.calls.find((entry) => entry.member === "setExpanded");
  assert.ok(expand);
  assert.equal(expand.args[0].relativePath, "assets");
  assert.equal(expand.args[0].expanded, true);
});

test("individual selection stays scoped to the active conversation", async (t) => {
  const f = fixture();
  t.after(() => f.panel.dispose());
  await f.panel.render(f.host, { sessionId: "session-one", title: "Financeiro" }, {});
  await settle();
  const row = byText(f.window, ".explorer-name", "README.md").closest(".explorer-row");
  const checkbox = row.querySelector(".explorer-check");
  checkbox.checked = true;
  checkbox.dispatchEvent(new f.window.Event("change", { bubbles: true }));
  await settle();
  const call = f.calls.find((entry) => entry.member === "setFileSelected");
  assert.ok(call);
  assert.equal(call.args[0].sessionId, "session-one");
  assert.equal(call.args[0].sourceId, "source-finance");
  assert.equal(call.args[0].relativePath, "README.md");
  assert.equal(call.args[0].selected, true);
});

test("folder checkbox, preferences and context preview reflect the budget", async (t) => {
  const f = fixture();
  t.after(() => f.panel.dispose());
  f.setPreview({
    selectedFiles: 40,
    estimatedBytes: 900000,
    estimatedChars: 300000,
    budgetChars: 200000,
    exceedsBudget: true,
    capped: false,
    missingSources: ["/home/user/removed"],
    sources: 2,
  });
  await f.panel.render(f.host, { sessionId: "session-one", title: "Financeiro" }, {});
  await settle();
  assert.match(f.host.textContent, /exceeds the budget/i);
  assert.match(f.host.textContent, /source\(s\) are unavailable/);

  const sourceCheckbox = f.host.querySelector(".explorer-source-head .explorer-check");
  sourceCheckbox.checked = false;
  sourceCheckbox.dispatchEvent(new f.window.Event("change", { bubbles: true }));
  await settle();
  const toggle = f.calls.find((entry) => entry.member === "setSourceSelected");
  assert.equal(toggle.args[0].selected, false);

  const hidden = Array.from(f.host.querySelectorAll(".explorer-toggle")).find((label) =>
    label.textContent.includes("Hidden"),
  );
  const hiddenBox = hidden.querySelector("input");
  hiddenBox.checked = true;
  hiddenBox.dispatchEvent(new f.window.Event("change", { bubbles: true }));
  await settle();
  const prefs = f.calls.find((entry) => entry.member === "setPreferences");
  assert.equal(prefs.args[0].showHidden, true);

  const excludes = f.host.querySelector(".explorer-excludes");
  excludes.value = "*.log, tmp/";
  excludes.dispatchEvent(new f.window.Event("change", { bubbles: true }));
  await settle();
  const extra = f.calls
    .filter((entry) => entry.member === "setPreferences")
    .find((entry) => entry.args[0].extraExcludes);
  assert.deepEqual(Array.from(extra.args[0].extraExcludes), ["*.log", "tmp/"]);

  byText(f.window, "button", "Context preview").click();
  await settle();
  const dialog = f.h.dialogs.find((entry) => entry.title === "Explorer context");
  assert.ok(dialog);
  const dialogText = dialog.content.map((node) => node.textContent).join(" ");
  assert.match(dialogText, /Budget: 200000 chars/);
  assert.match(dialogText, /will be truncated/);
  assert.match(dialogText, /Unavailable source: \/home\/user\/removed/);
});

test("native pickers add independent folders and files without merging sources", async (t) => {
  const f = fixture();
  t.after(() => f.panel.dispose());
  await f.panel.render(f.host, { sessionId: "session-one", title: "Financeiro" }, {});
  await settle();
  byText(f.window, ".explorer-toolbar button", "Add folder").click();
  await settle();
  const add = f.calls.find((entry) => entry.member === "addSource");
  assert.equal(add.args[0].path, "/home/user/documentos");
  assert.match(f.host.textContent, /documentos/);

  byText(f.window, ".explorer-toolbar button", "Add files").click();
  await settle();
  const files = f.calls.find((entry) => entry.member === "addSources");
  assert.deepEqual(files.args[0].paths, [
    "/home/user/configuracao.json",
    "/home/user/requisitos.txt",
  ]);
  assert.match(f.host.textContent, /configuracao\.json/);
  assert.match(f.host.textContent, /requisitos\.txt/);
});

test("a second conversation never receives the first conversation selection", async (t) => {
  const f = fixture();
  t.after(() => f.panel.dispose());
  await f.panel.render(f.host, { sessionId: "session-one", title: "Financeiro" }, {});
  await settle();
  assert.match(f.host.textContent, /PROJETO FINANCEIRO/);

  await f.panel.render(f.host, { sessionId: "session-two", title: "Outro" }, {});
  await settle();
  assert.match(f.host.textContent, /No sources yet/);
  assert.doesNotMatch(f.host.textContent, /PROJETO FINANCEIRO/);
  const stateCall = f.calls.filter((entry) => entry.member === "getState").at(-1);
  assert.equal(stateCall.args[0].sessionId, "session-two");
});

test("context menu exposes file and folder actions and file previews are readable", async (t) => {
  const f = fixture();
  t.after(() => f.panel.dispose());
  await f.panel.render(f.host, { sessionId: "session-one", title: "Financeiro" }, {});
  await settle();
  const row = byText(f.window, ".explorer-name", "index.php").closest(".explorer-row");
  row.dispatchEvent(
    new f.window.MouseEvent("contextmenu", { bubbles: true, clientX: 12, clientY: 12 }),
  );
  await wait();
  const menu = f.window.document.querySelector(".explorer-menu");
  assert.ok(menu);
  assert.match(menu.textContent, /Preview file/);
  assert.match(menu.textContent, /Remove from selection/);
  assert.match(menu.textContent, /Refresh source/);

  byText(f.window, ".explorer-name", "index.php").click();
  await settle();
  const preview = f.h.dialogs.find((entry) => entry.title === "index.php");
  assert.ok(preview);
  assert.match(preview.content.map((node) => node.textContent).join(" "), /olá/);

  const folderRow = byText(f.window, ".explorer-name", "assets").closest(".explorer-row");
  folderRow.dispatchEvent(
    new f.window.MouseEvent("contextmenu", { bubbles: true, clientX: 12, clientY: 12 }),
  );
  await wait();
  const folderMenu = f.window.document.querySelector(".explorer-menu");
  assert.match(folderMenu.textContent, /Select folder contents/);
  assert.match(folderMenu.textContent, /Unselect folder contents/);
});

test("Explorer is wired into the chat dock and shipped as its own asset", () => {
  assert.match(indexHtml, /src="\/explorer\.js"/);
  assert.match(appSource, /contextDrawer\("explorer"\)/);
  assert.match(appSource, /\["explorer", "Explorer"\]/);
  assert.match(appSource, /explorerDraftId/);
  assert.match(appSource, /transferContext/);
  assert.match(appCss, /\.explorer-panel/);
});
