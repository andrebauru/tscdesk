// Copyright (c) 2026 QuixiAI. MIT license.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { JSDOM } = require("jsdom");
const fs = require("node:fs");
const path = require("node:path");

const editorSource = fs.readFileSync(
  path.join(__dirname, "../../resources/ui/editor.js"),
  "utf8",
);
const explorerSource = fs.readFileSync(
  path.join(__dirname, "../../resources/ui/explorer.js"),
  "utf8",
);
const indexHtml = fs.readFileSync(
  path.join(__dirname, "../../resources/ui/index.html"),
  "utf8",
);
const appSource = fs.readFileSync(
  path.join(__dirname, "../../resources/ui/app.js"),
  "utf8",
);

const wait = (ms = 25) => new Promise((r) => setTimeout(r, ms));
const settle = async () => {
  for (let i = 0; i < 6; i += 1) await wait(20);
};

function createFixture() {
  const dom = new JSDOM(
    '<!doctype html><html><body><div id="workspace-split" class="workspace-split"><div id="page-host"></div><div id="editor-resizer"></div><aside id="editor-pane" hidden></aside></div><div id="toasts"></div></body></html>',
    { url: "https://app.localhost/chat", runScripts: "outside-only", pretendToBeVisual: true },
  );
  const window = dom.window;
  const disk = new Map([
    [
      "/ws/TESTE_RUNPOD.txt",
      {
        path: "/ws/TESTE_RUNPOD.txt",
        name: "TESTE_RUNPOD.txt",
        language: "plaintext",
        content: "RUNPOD EXECUTOU A ESCRITA COM SUCESSO",
        hash: "h1",
        lineEnding: "LF",
        encoding: "UTF-8",
        binary: false,
        mtimeMs: 100,
      },
    ],
    [
      "/ws/main.rs",
      {
        path: "/ws/main.rs",
        name: "main.rs",
        language: "rust",
        content: "fn main() {\n    println!(\"hello\");\n}\n",
        hash: "h2",
        lineEnding: "LF",
        encoding: "UTF-8",
        binary: false,
        mtimeMs: 100,
      },
    ],
    [
      "/ws/logo.png",
      {
        path: "/ws/logo.png",
        name: "logo.png",
        binary: true,
        error: "Binary or non-UTF-8 file cannot be edited as text.",
      },
    ],
  ]);
  const workspaceStates = new Map();
  const dialogs = [];
  const toasts = [];
  const layoutChanges = [];
  const calls = [];

  const el = (tag, attrs = {}, ...children) => {
    const node = window.document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (v === undefined || v === null || v === false) continue;
      if (k === "class") node.className = v;
      else if (k === "style") node.setAttribute("style", v);
      else if (k.startsWith("on")) node.addEventListener(k.slice(2).toLowerCase(), v);
      else if (k === "checked" || k === "hidden") node[k] = Boolean(v);
      else node.setAttribute(k, v === true ? "" : v);
    }
    for (const c of children.flat(Infinity)) {
      if (c === null || c === undefined || c === false) continue;
      node.append(c instanceof window.Node ? c : window.document.createTextNode(String(c)));
    }
    return node;
  };
  const icon = (n) => el("span", { class: "icon", "data-icon": n });
  const button = (label, fn, cls = "", ic) => {
    const b = el("button", { type: "button", class: cls }, ic ? icon(ic) : null, label);
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      fn(e);
    });
    return b;
  };
  const ib = (label, ic, fn) => {
    const b = button("", fn, "icon quiet", ic);
    b.setAttribute("aria-label", label);
    return b;
  };
  const dialog = (title, content) => {
    const d = el("dialog", {}, el("h3", {}, title), content);
    d.close = () => d.remove();
    window.document.body.append(d);
    dialogs.push({ title, node: d });
    return d;
  };

  const rpc = async (service, member, args = []) => {
    calls.push({ service, member, args });
    const body = args[0] || {};
    if (service === "Editor" && member === "openFile") {
      const p = body.path || `/ws/${body.relativePath}`;
      return structuredClone(disk.get(p) || { error: "Not found" });
    }
    if (service === "Editor" && member === "saveFile") {
      const cur = disk.get(body.path);
      if (!body.force && body.expectedHash && cur.hash !== body.expectedHash) {
        return { conflict: true, diskContent: cur.content, diskHash: cur.hash, diskMtimeMs: cur.mtimeMs };
      }
      cur.content = body.content;
      cur.hash = "h_" + Math.random().toString(36).slice(2, 8);
      cur.mtimeMs += 1;
      return { ok: true, path: body.path, hash: cur.hash, mtimeMs: cur.mtimeMs };
    }
    if (service === "Editor" && member === "statFile") {
      const cur = disk.get(body.path);
      if (!cur) return { exists: false };
      return {
        exists: true,
        path: cur.path,
        hash: cur.hash,
        mtimeMs: cur.mtimeMs,
        content: body.includeContent ? cur.content : undefined,
        lineEnding: cur.lineEnding,
      };
    }
    if (service === "Editor" && member === "getWorkspaceState") {
      return structuredClone(
        workspaceStates.get(body.workspaceKey) || {
          workspaceKey: body.workspaceKey,
          isOpen: false,
          layoutMode: "split",
          widthPx: 520,
          activePath: "",
          tabs: [],
        },
      );
    }
    if (service === "Editor" && member === "saveWorkspaceState") {
      workspaceStates.set(body.workspaceKey, structuredClone(body.state));
      return { ok: true };
    }
    return null;
  };

  window.eval(editorSource);
  window.eval(explorerSource);

  const editorPane = window.document.getElementById("editor-pane");
  const panel = window.TsEditorPanel.create({
    el,
    button,
    ib,
    icon,
    toast: (text, err) => toasts.push({ text, err }),
    rpc,
    dialog,
    onLayoutChange: (l) => {
      layoutChanges.push(l);
      editorPane.hidden = !l.isOpen || l.layoutMode === "chat-max";
    },
  });
  panel.mount(editorPane);
  panel.setSession("sess-1");

  return { window, panel, editorPane, disk, workspaceStates, dialogs, toasts, layoutChanges, calls, el, button, ib, icon, rpc, dialog };
}

test("editor is wired into index.html, explorer.js, and app.js outside #page-host", () => {
  assert.match(indexHtml, /href="\/editor\.css"/);
  assert.match(indexHtml, /src="\/editor\.js"/);
  assert.match(appSource, /id:\s*"editor-pane"/);
  assert.match(appSource, /id:\s*"workspace-split"/);
  assert.match(appSource, /onExternalFileChanged/);
  assert.match(explorerSource, /Open in Editor/);
});

test("opens tabs without duplicates, edits manually, saves with Ctrl+S, and detects binary files", async (t) => {
  const f = createFixture();
  t.after(() => f.panel.dispose());

  await f.panel.openFile({ path: "/ws/TESTE_RUNPOD.txt" });
  await f.panel.openFile({ path: "/ws/main.rs" });
  await f.panel.openFile({ path: "/ws/TESTE_RUNPOD.txt" }); // activates existing tab
  await settle();

  const tabs = f.editorPane.querySelectorAll(".editor-tab");
  assert.equal(tabs.length, 2);
  assert.equal(f.editorPane.querySelector(".editor-tab.active .editor-tab-name").textContent, "TESTE_RUNPOD.txt");

  const ta = f.editorPane.querySelector(".editor-input");
  ta.value = "RUNPOD EXECUTOU A ESCRITA COM SUCESSO\nEDICAO MANUAL";
  ta.dispatchEvent(new f.window.Event("input", { bubbles: true }));
  await settle();
  assert.ok(f.editorPane.querySelector(".editor-tab.active .editor-dirty-dot"));

  ta.dispatchEvent(new f.window.KeyboardEvent("keydown", { key: "s", ctrlKey: true, bubbles: true }));
  await settle();
  assert.equal(f.disk.get("/ws/TESTE_RUNPOD.txt").content, "RUNPOD EXECUTOU A ESCRITA COM SUCESSO\nEDICAO MANUAL");
  assert.equal(f.editorPane.querySelector(".editor-tab.active .editor-dirty-dot"), null);

  await f.panel.openFile({ path: "/ws/logo.png" });
  await settle();
  assert.match(f.editorPane.textContent, /Binary or non-UTF-8 file/);
});

test("handles external AI changes: auto-reloads when clean and shows diff/conflict banner when dirty", async (t) => {
  const f = createFixture();
  t.after(() => f.panel.dispose());

  await f.panel.openFile({ path: "/ws/TESTE_RUNPOD.txt" });
  await settle();

  // Clean external update -> auto-reloads
  f.disk.get("/ws/TESTE_RUNPOD.txt").content = "ATUALIZADO PELA IA";
  f.disk.get("/ws/TESTE_RUNPOD.txt").hash = "h_ai_1";
  await f.panel.handleExternalChange({ path: "/ws/TESTE_RUNPOD.txt" });
  await settle();
  assert.equal(f.editorPane.querySelector(".editor-input").value, "ATUALIZADO PELA IA");

  // Dirty local edit + external update -> shows conflict banner without overwriting
  const ta = f.editorPane.querySelector(".editor-input");
  ta.value = "MINHA EDICAO LOCAL";
  ta.dispatchEvent(new f.window.Event("input", { bubbles: true }));
  await settle();

  f.disk.get("/ws/TESTE_RUNPOD.txt").content = "CONFLITO DA IA";
  f.disk.get("/ws/TESTE_RUNPOD.txt").hash = "h_ai_2";
  await f.panel.handleExternalChange({ path: "/ws/TESTE_RUNPOD.txt" });
  await settle();

  const banner = f.editorPane.querySelector(".editor-conflict-banner");
  assert.ok(banner);
  assert.equal(f.editorPane.querySelector(".editor-input").value, "MINHA EDICAO LOCAL");
});

test("persists open tabs, active tab, layout, and unsaved drafts across restart", async (t) => {
  const f = createFixture();
  t.after(() => f.panel.dispose());

  await f.panel.loadWorkspace("sess-1", "D:\\ws");
  await f.panel.openFile({ path: "/ws/TESTE_RUNPOD.txt" });
  await f.panel.openFile({ path: "/ws/main.rs" });
  const ta = f.editorPane.querySelector(".editor-input");
  ta.value = "fn main() { /* draft */ }";
  ta.dispatchEvent(new f.window.Event("input", { bubbles: true }));
  f.panel.setWidth(640);
  await wait(450);

  // Simulate restart with a fresh panel reading from workspaceStates
  const panel2 = windowTsPanelClone(f);
  t.after(() => panel2.dispose());
  await panel2.loadWorkspace("sess-1", "D:\\ws");
  await settle();

  const st = panel2.getState();
  assert.equal(st.isOpen, true);
  assert.equal(st.widthPx, 640);
  assert.equal(st.tabs.length, 2);
  assert.equal(st.activePath, "/ws/main.rs");
  assert.equal(st.tabs[1].content, "fn main() { /* draft */ }");
  assert.equal(st.tabs[1].dirty, true);
});

function windowTsPanelClone(f) {
  const pane2 = f.window.document.createElement("aside");
  f.window.document.body.append(pane2);
  const p2 = f.window.TsEditorPanel.create({
    el: f.el,
    button: f.button,
    ib: f.ib,
    icon: f.icon,
    toast: (text, err) => f.toasts.push({ text, err }),
    rpc: f.rpc,
    dialog: f.dialog,
    onLayoutChange: (l) => {
      pane2.hidden = !l.isOpen || l.layoutMode === "chat-max";
    },
  });
  p2.mount(pane2);
  return p2;
}
