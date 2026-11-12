// Copyright (c) 2026 QuixiAI. MIT license.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { randomBytes } = require("node:crypto");
const { JSDOM } = require("jsdom");
const ui = path.join(__dirname, "../../resources/ui");
const wait = () => new Promise(resolve => setTimeout(resolve, 35));

async function fixture({ configured = true, paused = false, kind = "task", setResult, setError } = {}) {
  const dom = new JSDOM(fs.readFileSync(path.join(ui, "index.html"), "utf8"), {
    url: "https://app.localhost/new", runScripts: "outside-only", pretendToBeVisual: true,
  });
  const w = dom.window, calls = [], pollTimers = new Map();
  w.TextEncoder = TextEncoder;
  w.TextDecoder = TextDecoder;
  w.matchMedia = () => ({ matches: true, addEventListener() {} });
  w.HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  w.HTMLDialogElement.prototype.close = function () {
    this.open = false; this.dispatchEvent(new w.Event("close"));
  };
  w.fetch = async () => { throw new Error("Live network is forbidden in UI tests"); };
  // Drive only the local engine polling clock; leave other real frontend timers alone.
  const realTimeout = w.setTimeout.bind(w), realClear = w.clearTimeout.bind(w);
  let timerId = 100000;
  w.setTimeout = (fn, ms, ...args) => {
    if (ms !== 4000) return realTimeout(fn, ms, ...args);
    const id = ++timerId; pollTimers.set(id, fn); return id;
  };
  w.clearTimeout = id => { pollTimers.delete(id); realClear(id); };
  const provider = {
    id: "runpod-ollama", name: "RunPod Serverless", category: "cloud", configured,
    apiFormat: "runpod", endpointId: configured ? "endpoint-fixture" : "",
    selectedModel: configured ? "qwen2.5-coder:7b" : "", apiKeyMasked: configured ? "••••fixture" : "",
    models: configured ? [{ id: "qwen2.5-coder:7b", label: "qwen2.5-coder:7b" }] : [],
  };
  // Accessing a recoverable key is forbidden, even if a future snapshot accidentally adds one.
  for (const name of ["apiKey", "apiKeyRaw", "api_key_raw"])
    Object.defineProperty(provider, name, { get() { throw new Error("Frontend attempted to read a secret"); } });
  const providers = {
    configuredCount: configured ? 1 : 0, hasConfiguredProvider: configured,
    activeProvider: configured ? provider.id : "", activeModel: provider.selectedModel,
    providers: [provider],
  };
  const sessions = [{ sessionId: "fixture-one", title: "Existing conversation", isRunning: false,
    turnRunning: false, isArchived: false, cwd: "", lastActivityAt: 100, bufferedMessages: [] }];
  let agent = { paused, status: paused ? "paused" : "available" }, pendingPoll;
  let delayNextPoll = false, submittedKey = false;
  const record = (service, member, args) => {
    // Never retain credentials in the test call log or in assertion snapshots.
    calls.push({ service, member, args: Array.from(args, arg => {
      if (!arg || typeof arg !== "object") return arg;
      const safe = { ...arg };
      if (Object.hasOwn(safe, "apiKey")) safe.apiKey = safe.apiKey ? "[draft supplied]" : "";
      return safe;
    }) });
  };
  w.quixi = {
    origin: "https://app.localhost", copy: async () => {},
    api: async (route, body) => {
      calls.push({ route });
      if (route === "/api/quixi/python/settings") return { installer: "pip", useVenv: true };
      if (route === "/api/quixi/providers") return providers;
      if (route === "/api/quixi/providers/complete-onboarding") return providers;
      throw new Error("Unexpected frontend API: " + route);
    },
    rpc: async (service, member, args = []) => {
      record(service, member, args);
      if (service === "Providers") {
        if (member === "getAgentState") {
          if (delayNextPoll) {
            delayNextPoll = false;
            return new Promise(resolve => { pendingPoll = resolve; });
          }
          return { ...agent };
        }
        if (member === "setAgentState") {
          if (setError) return { error: setError };
          agent = setResult || { paused: args[0].paused, status: args[0].paused ? "paused" : "available" };
          return { ...agent };
        }
        if (member === "testProvider") return { ok: true, message: "Health disponível; inferência não testada." };
        if (member === "saveProvider") {
          const config = args[0];
          submittedKey ||= Boolean(config.apiKey);
          if (config.endpointId !== undefined) provider.endpointId = config.endpointId;
          if (config.apiKey) provider.apiKeyMasked = "••••replacement";
          if (config.selectedModel) provider.selectedModel = config.selectedModel;
          provider.configured = true;
          provider.models = [{ id: provider.selectedModel, label: provider.selectedModel }];
          providers.hasConfiguredProvider = true; providers.configuredCount = 1;
          providers.activeProvider = provider.id; providers.activeModel = provider.selectedModel;
          return providers;
        }
        throw new Error("Unexpected Providers RPC: " + member);
      }
      if (member === "getAll") return service === (kind === "code" ? "LocalSessions" : "LocalAgentModeSessions") ? structuredClone(sessions) : [];
      if (member === "getAllSpaces" || member === "getDefaultWorkspaceFolders" || member === "pendingInteractions" || member === "resourceUpdates") return [];
      if (member === "getSession") return structuredClone(sessions.find(s => s.sessionId === args[0]));
      if (member === "getPendingMessages" || member === "getToolPermissions" || member === "listSubagents" || member === "listJobs") return [];
      if (member === "start") {
        const s = { ...sessions[0], sessionId: "fixture-new", title: args[0].title, bufferedMessages: [] };
        sessions.push(s); return structuredClone(s);
      }
      if (member === "sendMessageV2") {
        const s = sessions.find(s => s.sessionId === args[0]);
        s.isRunning = s.turnRunning = true;
        s.bufferedMessages.push({ type: "user", message: { content: [{ type: "text", text: args[1].text }] } });
        agent = { paused: false, status: "processing" };
        return { dispatched: true };
      }
      if (member === "stop") { sessions[0].isRunning = sessions[0].turnRunning = false; return true; }
      if (member === "getMessages") return sessions[0].bufferedMessages;
      return null;
    },
  };
  for (const file of ["workspace-workflows.js", "schedule-workflows.js", "capability-workflows.js", "chat-workflows.js", "app.js"])
    w.eval(fs.readFileSync(path.join(ui, file), "utf8"));
  const style = w.document.createElement("style"); style.textContent = fs.readFileSync(path.join(ui, "app.css"), "utf8");
  w.document.head.append(style);
  await wait();
  const click = async label => {
    const b = [...w.document.querySelectorAll("button")].find(b => b.textContent.trim() === label || b.getAttribute("aria-label") === label);
    assert.ok(b, "Missing button: " + label); b.click(); await wait(); return b;
  };
  const type = (selector, value) => {
    const input = w.document.querySelector(selector); assert.ok(input, selector);
    input.value = value; input.dispatchEvent(new w.Event("input", { bubbles: true })); return input;
  };
  const event = value => { agent = value; w.dispatchEvent(new w.CustomEvent("quixi:event", { detail: { channel: "quixi:agent-state", payload: value } })); };
  const openProvider = async () => { await click("Providers" + (providers.configuredCount ? " · " + providers.configuredCount : "")); await click("Configure"); };
  return { w, calls, provider, sessions, click, type, event, openProvider, pollTimers,
    hasSubmittedKey: () => submittedKey,
    delayPoll: () => { delayNextPoll = true; },
    finishPoll: value => { assert.ok(pendingPoll); pendingPoll(value); },
    close: () => dom.window.close() };
}

const providerCalls = f => f.calls.filter(c => c.service === "Providers" && ["saveProvider", "testProvider"].includes(c.member));
const sends = f => f.calls.filter(c => c.member === "sendMessageV2");
const commands = f => f.calls.filter(c => c.member === "setAgentState");

test("dynamic RunPod directory and editor use Endpoint ID and model, with only a blank password draft", async () => {
  const f = await fixture();
  try {
    await f.openProvider();
    const d = f.w.document.querySelector('dialog[aria-label="RunPod Serverless"]');
    assert.ok(d); assert.match(d.textContent, /queue-based.*cold start.*resposta.*completa.*streaming.*tools.*health.*inferência/s);
    assert.equal(d.querySelector('input[type="url"]'), null);
    assert.equal(d.querySelector("select"), null);
    assert.equal(d.querySelector('[name="endpointId"]').value, "endpoint-fixture");
    assert.equal(d.querySelector('[name="selectedModel"]').value, "qwen2.5-coder:7b");
    const key = d.querySelector('[name="apiKey"]');
    assert.equal(key.type, "password"); assert.equal(key.value, "");
    assert.match(key.placeholder, /••••fixture.*branco/);
    assert.equal(providerCalls(f).length, 0);
    assert.equal(f.w.localStorage.length, 0); assert.equal(f.w.sessionStorage.length, 0);
  } finally { f.close(); }
});

test("RunPod blank-key save preserves the mask, accepts endpoint/model edits and never discovers or tests models", async () => {
  const f = await fixture();
  try {
    await f.openProvider();
    f.type('[name="endpointId"]', "  another-endpoint  ");
    f.type('[name="selectedModel"]', "  qwen2.5-coder:14b  ");
    await f.click("Save & use provider");
    const saved = providerCalls(f);
    assert.equal(saved.length, 1); assert.equal(saved[0].member, "saveProvider");
    assert.deepEqual(saved[0].args[0], { providerId: "runpod-ollama", endpointId: "another-endpoint", apiKey: "", selectedModel: "qwen2.5-coder:14b", useEndpointDefault: false, setAsActive: true });
    assert.equal(f.provider.apiKeyMasked, "••••fixture");
    await f.click("Configure");
    assert.equal(f.w.document.querySelector('[name="apiKey"]').value, "");
    assert.match(f.w.document.querySelector('[name="apiKey"]').placeholder, /••••fixture/);
    assert.equal(f.calls.some(c => /providers\/(save|test)/.test(c.route || "")), false);
  } finally { f.close(); }
});

test("RunPod connection check is explicit-only, uses the health RPC and never logs or stores the key draft", async () => {
  const f = await fixture();
  try {
    await f.openProvider();
    const draft = randomBytes(20).toString("hex");
    const key = f.type('[name="apiKey"]', draft);
    assert.equal(providerCalls(f).length, 0);
    f.w.dispatchEvent(new f.w.Event("focus")); await wait();
    assert.equal(providerCalls(f).length, 0);
    await f.click("Testar conexão");
    const checked = providerCalls(f);
    assert.equal(checked.length, 1); assert.equal(checked[0].member, "testProvider");
    assert.equal(checked[0].args[0].apiKey, "[draft supplied]");
    assert.equal(Object.hasOwn(checked[0].args[0], "baseUrl"), false);
    assert.equal(Object.hasOwn(checked[0].args[0], "apiFormat"), false);
    assert.equal(Object.hasOwn(checked[0].args[0], "refreshModels"), false);
    assert.match(f.w.document.querySelector('dialog[aria-label="RunPod Serverless"] [role="status"]').textContent, /Health.*inferência não testada/);
    assert.equal(f.w.document.body.outerHTML.includes(draft), false);
    assert.equal(JSON.stringify(f.calls).includes(draft), false);
    assert.equal(f.w.localStorage.length, 0); assert.equal(f.w.sessionStorage.length, 0);
    await f.click("Save & use provider");
    assert.equal(f.hasSubmittedKey(), true); assert.equal(key.value, "");
    await f.click("Configure");
    assert.equal(f.w.document.querySelector('[name="apiKey"]').value, "");
    assert.match(f.w.document.querySelector('[name="apiKey"]').placeholder, /••••replacement/);
  } finally { f.close(); }
});

test("RunPod health test does not require a model, while saving still does", async () => {
  const f = await fixture();
  try {
    await f.openProvider();
    f.type('[name="selectedModel"]', "");
    await f.click("Testar conexão");
    assert.equal(providerCalls(f).length, 1);
    assert.equal(providerCalls(f)[0].member, "testProvider");
    await f.click("Save & use provider");
    assert.equal(providerCalls(f).length, 1);
    assert.match(f.w.document.querySelector('dialog[aria-label="RunPod Serverless"] [role="status"]').textContent, /Model ID/);
  } finally { f.close(); }
});

test("RunPod endpoint-default mode disables Model ID and saves it empty", async () => {
  const f = await fixture();
  try {
    await f.openProvider();
    const checkbox = f.w.document.querySelector('[name="useEndpointDefault"]');
    checkbox.click();
    const model = f.w.document.querySelector('[name="selectedModel"]');
    assert.equal(model.disabled, true);
    assert.equal(model.required, false);
    await f.click("Save & use provider");
    const saved = providerCalls(f);
    assert.equal(saved.length, 1);
    assert.deepEqual(saved[0].args[0], {
      providerId: "runpod-ollama", endpointId: "endpoint-fixture", apiKey: "",
      selectedModel: "", useEndpointDefault: true, setAsActive: true,
    });
  } finally { f.close(); }
});

test("unconfigured RunPod defaults to an empty endpoint and exact model; validation does not perform health requests", async () => {
  const f = await fixture({ configured: false });
  try {
    await f.openProvider();
    assert.equal(f.w.document.querySelector('[name="endpointId"]').value, "");
    assert.equal(f.w.document.querySelector('[name="selectedModel"]').value, "qwen2.5-coder:7b");
    await f.click("Testar conexão"); await f.click("Save & use provider");
    assert.equal(providerCalls(f).length, 0);
    assert.match(f.w.document.querySelector('dialog[aria-label="RunPod Serverless"] [role="status"]').textContent, /Endpoint ID/);
  } finally { f.close(); }
});

test("RunPod model picker does not refresh a remote catalog or health on opening or hidden refresh", async () => {
  const f = await fixture();
  try {
    await f.click("Choose model");
    const refresh = [...f.w.document.querySelectorAll("button")].find(b => b.textContent === "Refresh models");
    assert.ok(refresh.hidden); assert.equal(f.w.getComputedStyle(refresh).display, "none");
    refresh.click(); await wait();
    assert.equal(providerCalls(f).length, 0);
    assert.equal(f.calls.some(c => c.route === "/api/quixi/providers/test"), false);
    await f.click("Configure RunPod");
    assert.equal(f.w.document.querySelector('[name="apiKey"]').value, "");
  } finally { f.close(); }
});

test("global pause/resume buttons reflect backend states and never clear running transcript or Stop locally", async () => {
  const f = await fixture({ setResult: { paused: true, status: "cancelling", message: "Aguardando cancelamento remoto" } });
  try {
    f.sessions[0].isRunning = true;
    f.sessions[0].bufferedMessages = [{ type: "assistant", message: { content: [{ type: "text", text: "Keep the recorded response" }] } }];
    await f.click("Existing conversation");
    assert.equal(f.w.document.querySelector("#agent-status").textContent, "IA disponível");
    await f.click("Pausar IA");
    assert.deepEqual(commands(f)[0].args, [{ paused: true }]);
    assert.equal(f.w.document.querySelector("#agent-status").textContent, "Cancelando");
    assert.equal(f.w.document.querySelector("#stop-button").hidden, false);
    assert.match(f.w.document.querySelector("#conversation").textContent, /Keep the recorded response/);
    assert.equal(f.calls.some(c => c.member === "stop"), false);
    f.event({ paused: true, status: "cancel_error", message: "Remote cancellation could not be confirmed" });
    assert.equal(f.w.document.querySelector("#agent-status").textContent, "Erro ao cancelar");
    assert.match(f.w.document.querySelector("#agent-status").title, /could not be confirmed/);
    f.event({ paused: true, status: "paused" });
    assert.equal(f.w.document.querySelector("#agent-status").textContent, "IA pausada");
    f.event({ paused: false, status: "processing" });
    assert.equal(f.w.document.querySelector("#agent-status").textContent, "IA processando");
    f.event({ paused: false, status: "available" });
    assert.equal(f.w.document.querySelector("#agent-status").textContent, "IA disponível");
  } finally { f.close(); }
});

for (const kind of ["task", "code"])
  test(`paused ${kind} composer blocks ordinary prompts but permits only trimmed isolated ASCII FINAL/INICIAR`, async () => {
    const f = await fixture({ paused: true, kind });
    try {
      await f.click("Existing conversation");
      const input = f.type("#prompt", "Do regular work");
      assert.equal(input.disabled, false); assert.equal(f.w.document.querySelector("#send-button").disabled, true);
      input.dispatchEvent(new f.w.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })); await wait();
      assert.equal(sends(f).length, 0); assert.equal(input.value, "Do regular work");
      for (const text of ["FINAL please", "please INICIAR", "INICIAR\nmore", "İNİCİAR", "/goal paid inference"]) {
        f.type("#prompt", text); input.dispatchEvent(new f.w.KeyboardEvent("keydown", { key: "Enter", bubbles: true })); await wait();
      }
      assert.equal(commands(f).length, 0); assert.equal(sends(f).length, 0);
      f.type("#prompt", " \n fInAl \t");
      assert.equal(f.w.document.querySelector("#send-button").disabled, false);
      await f.click("Send");
      assert.deepEqual(commands(f)[0].args, [{ paused: true }]); assert.equal(input.value, "");
      f.type("#prompt", " \t iNiCiAr \n"); await f.click("Send");
      assert.deepEqual(commands(f)[1].args, [{ paused: false }]);
      assert.equal(f.w.document.querySelector("#agent-status").textContent, "IA disponível");
      f.type("#prompt", "Regular work now"); await f.click("Send");
      assert.equal(sends(f).length, 1);
      assert.equal(sends(f)[0].service, kind === "code" ? "LocalSessions" : "LocalAgentModeSessions");
      assert.equal(f.w.document.querySelector("#agent-status").textContent, "IA processando");
    } finally { f.close(); }
  });

test("resumed agent admits a new prompt while old cancellations settle", async () => {
  const f = await fixture({ paused: true });
  try {
    f.event({ paused: false, status: "cancelling", message: "Old cancelled requests are settling." });
    f.type("#prompt", "New deliberate question");
    assert.equal(f.w.document.getElementById("send-button").disabled, false);
    await f.click("Send");
    assert.equal(sends(f).length, 1);
    assert.equal(providerCalls(f).length, 0);
  } finally { f.close(); }
});

test("isolated commands work with no provider and do not create or send a conversation", async () => {
  const f = await fixture({ configured: false, paused: true });
  try {
    f.type("#prompt", "iniciar"); await f.click("Send");
    await f.click("Pausar IA"); await f.click("Retomar IA");
    assert.deepEqual(commands(f).map(c => c.args[0].paused), [false, true, false]);
    assert.equal(sends(f).length, 0); assert.equal(f.calls.some(c => c.member === "start"), false);
    assert.equal(providerCalls(f).length, 0);
  } finally { f.close(); }
});

test("attachments prevent command recognition, remain preserved while paused and send as ordinary content after resume", async () => {
  const f = await fixture({ paused: true });
  try {
    const file = { name: "notes.txt", type: "text/plain", size: 4, arrayBuffer: async () => new TextEncoder().encode("note").buffer };
    const picker = f.w.document.querySelector("#attachment-input");
    Object.defineProperty(picker, "files", { value: [file], configurable: true });
    picker.dispatchEvent(new f.w.Event("change", { bubbles: true })); await wait();
    const input = f.type("#prompt", "INICIAR");
    assert.equal(f.w.document.querySelector("#send-button").disabled, true);
    input.dispatchEvent(new f.w.KeyboardEvent("keydown", { key: "Enter", bubbles: true })); await wait();
    assert.equal(commands(f).length, 0); assert.equal(sends(f).length, 0);
    assert.match(f.w.document.querySelector("#attachment-chips").textContent, /notes.txt/);
    await f.click("Retomar IA"); await f.click("Send");
    assert.equal(commands(f).length, 1); assert.equal(sends(f).length, 1);
    assert.equal(sends(f)[0].args[1].text, "INICIAR");
    assert.equal(sends(f)[0].args[1].attachments[0].name, "notes.txt");
  } finally { f.close(); }
});

test("failed global commands keep the draft and authoritative state instead of claiming cancellation", async () => {
  const f = await fixture({ setError: "Engine cancellation unavailable" });
  try {
    f.type("#prompt", "FINAL"); await f.click("Send");
    assert.equal(f.w.document.querySelector("#prompt").value, "FINAL");
    assert.equal(f.w.document.querySelector("#agent-status").textContent, "IA disponível");
    assert.match(f.w.document.querySelector("#toasts").textContent, /cancellation unavailable/);
    assert.equal(sends(f).length, 0);
  } finally { f.close(); }
});

test("local state polling cannot overwrite a newer user command and stops while the page is closed or hidden", async () => {
  const f = await fixture();
  try {
    assert.equal(f.pollTimers.size, 1);
    f.delayPoll();
    f.w.dispatchEvent(new f.w.Event("focus")); await wait();
    await f.click("Pausar IA");
    f.finishPoll({ paused: false, status: "available" }); await wait();
    assert.equal(f.w.document.querySelector("#agent-status").textContent, "IA pausada");
    const callback = [...f.pollTimers.values()][0]; f.pollTimers.clear();
    await callback();
    assert.equal(f.pollTimers.size, 1); assert.equal(providerCalls(f).length, 0);
    Object.defineProperty(f.w.document, "hidden", { value: true, configurable: true });
    f.w.document.dispatchEvent(new f.w.Event("visibilitychange"));
    assert.equal(f.pollTimers.size, 0);
    Object.defineProperty(f.w.document, "hidden", { value: false, configurable: true });
    f.w.document.dispatchEvent(new f.w.Event("visibilitychange")); await wait();
    assert.equal(f.pollTimers.size, 1);
    f.w.dispatchEvent(new f.w.Event("pagehide"));
    assert.equal(f.pollTimers.size, 0);
    assert.equal(providerCalls(f).length, 0);
  } finally { f.close(); }
});
