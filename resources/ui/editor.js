/* Copyright (c) 2026 QuixiAI. MIT license. Integrated Microsoft Monaco Editor
   with offline local bundle, TOML Monarch grammar, multi-tab management,
   Ctrl+S atomic save, encoding safety, diff conflict resolution, and layout persistence. */
(() => {
  "use strict";

  const SAVE_DEBOUNCE_MS = 350;
  const STAT_POLL_MS = 2500;
  let monacoPromise = null;

  function registerTomlLanguage(monaco) {
    if (monaco.languages.getLanguages().some((l) => l.id === "toml")) return;
    monaco.languages.register({
      id: "toml",
      extensions: [".toml"],
      filenames: ["Cargo.toml", "Cargo.lock", "pyproject.toml"],
      aliases: ["TOML", "toml"],
    });
    monaco.languages.setLanguageConfiguration("toml", {
      comments: { lineComment: "#" },
      brackets: [
        ["{", "}"],
        ["[", "]"],
        ["(", ")"],
      ],
      autoClosingPairs: [
        { open: "{", close: "}" },
        { open: "[", close: "]" },
        { open: "(", close: ")" },
        { open: '"', close: '"' },
        { open: "'", close: "'" },
      ],
      surroundingPairs: [
        { open: "{", close: "}" },
        { open: "[", close: "]" },
        { open: "(", close: ")" },
        { open: '"', close: '"' },
        { open: "'", close: "'" },
      ],
    });
    monaco.languages.setMonarchTokensProvider("toml", {
      defaultToken: "",
      tokenPostfix: ".toml",
      tokenizer: {
        root: [
          [/#.*$/, "comment"],
          [/^\s*\[\[?[^\]\r\n]+\]?\]/, "type.identifier"],
          [/^\s*[A-Za-z0-9_.-]+(?=\s*=)/, "key"],
          [/"""/, "string", "@mlStringDouble"],
          [/'''/, "string", "@mlStringSingle"],
          [/"([^"\\]|\\.)*$/, "string.invalid"],
          [/"/, "string", "@stringDouble"],
          [/'[^']*'/, "string"],
          [/\b(?:true|false)\b/, "keyword"],
          [/\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?)?/, "number"],
          [/0x[0-9a-fA-F_]+|0o[0-7_]+|0b[01_]+/, "number.hex"],
          [/[+-]?(?:\d[\d_]*(?:\.[\d_]+)?(?:[eE][+-]?\d[\d_]*)?|inf|nan)\b/, "number"],
          [/[{}()\[\]]/, "@brackets"],
          [/[=,]/, "delimiter"],
        ],
        stringDouble: [
          [/[^\\"]+/, "string"],
          [/\\./, "string.escape"],
          [/"/, "string", "@pop"],
        ],
        mlStringDouble: [
          [/[^\\"]+/, "string"],
          [/\\./, "string.escape"],
          [/"""/, "string", "@pop"],
          [/"/, "string"],
        ],
        mlStringSingle: [
          [/[^']+/, "string"],
          [/'''/, "string", "@pop"],
          [/'/, "string"],
        ],
      },
    });
  }

  function defineThemes(monaco) {
    monaco.editor.defineTheme("tscide-dark", {
      base: "vs-dark",
      inherit: true,
      rules: [
        { token: "key", foreground: "9cdcfe" },
        { token: "type.identifier", foreground: "d97757", fontStyle: "bold" },
      ],
      colors: {
        "editor.background": "#161618",
        "editor.lineHighlightBackground": "#1f2024",
        "editorGutter.background": "#161618",
      },
    });
    monaco.editor.defineTheme("tscide-light", {
      base: "vs",
      inherit: true,
      rules: [
        { token: "key", foreground: "0451a5" },
        { token: "type.identifier", foreground: "b8502e", fontStyle: "bold" },
      ],
      colors: {
        "editor.background": "#ffffff",
      },
    });
  }

  function currentMonacoTheme() {
    return document.documentElement.dataset.theme === "light"
      ? "tscide-light"
      : "tscide-dark";
  }

  function loadMonaco() {
    if (window.monaco?.editor) return Promise.resolve(window.monaco);
    if (monacoPromise) return monacoPromise;
    if (typeof window.Worker === "undefined" || typeof navigator === "undefined" || /jsdom/i.test(navigator.userAgent || "")) {
      return Promise.resolve(null);
    }
    monacoPromise = new Promise((resolve) => {
      const origin = window.location.origin;
      window.MonacoEnvironment = {
        getWorkerUrl() {
          const code = `self.MonacoEnvironment = { baseUrl: '${origin}/vendor/monaco-editor/min/' }; importScripts('${origin}/vendor/monaco-editor/min/vs/base/worker/workerMain.js');`;
          return URL.createObjectURL(new Blob([code], { type: "text/javascript" }));
        },
      };
      const initWithRequire = () => {
        try {
          window.require.config({
            paths: { vs: `${origin}/vendor/monaco-editor/min/vs` },
          });
          window.require(["vs/editor/editor.main"], () => {
            if (window.monaco?.editor) {
              registerTomlLanguage(window.monaco);
              defineThemes(window.monaco);
              resolve(window.monaco);
            } else {
              resolve(null);
            }
          }, () => resolve(null));
        } catch (_) {
          resolve(null);
        }
      };
      if (typeof window.require === "function") {
        initWithRequire();
        return;
      }
      const script = document.createElement("script");
      script.src = "/vendor/monaco-editor/min/vs/loader.js";
      script.onload = initWithRequire;
      script.onerror = () => resolve(null);
      document.head.appendChild(script);
    });
    return monacoPromise;
  }

  function detectLanguageFromName(name, fallback = "plaintext") {
    const lower = String(name || "").toLowerCase();
    if (lower === "cargo.toml" || lower === "cargo.lock" || lower.endsWith(".toml")) return "toml";
    if (lower.endsWith(".rs")) return "rust";
    if (lower.endsWith(".json") || lower.endsWith(".jsonc")) return "json";
    if (lower.endsWith(".yaml") || lower.endsWith(".yml")) return "yaml";
    if (lower.endsWith(".js") || lower.endsWith(".jsx") || lower.endsWith(".mjs") || lower.endsWith(".cjs")) return "javascript";
    if (lower.endsWith(".ts") || lower.endsWith(".tsx") || lower.endsWith(".mts") || lower.endsWith(".cts")) return "typescript";
    if (lower.endsWith(".py") || lower.endsWith(".pyw")) return "python";
    if (lower.endsWith(".html") || lower.endsWith(".htm")) return "html";
    if (lower.endsWith(".css") || lower.endsWith(".scss")) return "css";
    if (lower.endsWith(".md") || lower.endsWith(".markdown")) return "markdown";
    if (lower.endsWith(".sh") || lower.endsWith(".bash") || lower.endsWith(".zsh")) return "shell";
    if (lower.endsWith(".ps1") || lower.endsWith(".psm1")) return "powershell";
    if (lower.endsWith(".sql")) return "sql";
    if (lower.endsWith(".xml") || lower.endsWith(".svg")) return "xml";
    if (lower.endsWith(".php")) return "php";
    if (lower.endsWith(".go")) return "go";
    if (lower.endsWith(".c") || lower.endsWith(".h")) return "c";
    if (lower.endsWith(".cpp") || lower.endsWith(".hpp") || lower.endsWith(".cc")) return "cpp";
    return fallback || "plaintext";
  }

  function simpleLineDiff(localText, diskText) {
    const a = String(localText || "").split("\n");
    const b = String(diskText || "").split("\n");
    const max = Math.max(a.length, b.length);
    const out = [];
    for (let i = 0; i < max; i += 1) {
      const la = a[i];
      const lb = b[i];
      if (la === lb) out.push(`  ${la ?? ""}`);
      else {
        if (la !== undefined) out.push(`- ${la}`);
        if (lb !== undefined) out.push(`+ ${lb}`);
      }
    }
    return out.join("\n");
  }

  function create(deps) {
    const { el, button, ib, icon, toast, rpc, dialog, onLayoutChange } = deps;
    let host = null;
    let sessionId = "";
    let workspaceKey = "default";
    let isOpen = false;
    let layoutMode = "split";
    let widthPx = 560;
    let wordWrap = false;
    let minimapEnabled = false;
    let tabs = [];
    let activePath = "";
    let saveTimer = null;
    let pollTimer = null;
    let dragPath = null;
    let monacoApi = null;
    let monacoInstance = null;
    let monacoHostNode = null;
    let suppressModelChange = false;

    const activeTab = () => tabs.find((t) => t.path === activePath) || null;
    const normalizePathKey = (p) =>
      String(p || "")
        .replace(/^\\\\\?\\/, "")
        .replace(/\\/g, "/")
        .toLowerCase();
    const samePath = (a, b) => Boolean(a && b && normalizePathKey(a) === normalizePathKey(b));

    const schedulePersist = () => {
      clearTimeout(saveTimer);
      saveTimer = setTimeout(() => {
        void persistNow();
      }, SAVE_DEBOUNCE_MS);
    };

    const persistNow = async () => {
      if (!workspaceKey) return;
      try {
        await rpc("Editor", "saveWorkspaceState", [
          {
            workspaceKey,
            state: {
              workspaceKey,
              isOpen,
              layoutMode,
              widthPx: Math.round(widthPx),
              activePath,
              tabs: tabs.map((t) => ({
                path: t.path,
                name: t.name,
                relativePath: t.relativePath || "",
                sourceId: t.sourceId || "",
                language: t.language || "plaintext",
                dirty: Boolean(t.dirty),
                draftContent: t.dirty ? t.content : null,
                baseHash: t.baseHash || "",
                lineEnding: t.lineEnding || "LF",
                scrollTop: Number(t.scrollTop) || 0,
                cursorStart: Number(t.cursorStart) || 0,
                cursorEnd: Number(t.cursorEnd) || 0,
              })),
            },
          },
        ]);
      } catch (_) {}
    };

    const notifyLayout = () => {
      onLayoutChange?.({ isOpen, layoutMode, widthPx });
    };

    const disposeMonaco = () => {
      if (monacoInstance) {
        try {
          monacoInstance.dispose();
        } catch (_) {}
        monacoInstance = null;
        monacoHostNode = null;
      }
    };

    const saveTab = async (tab = activeTab(), force = false) => {
      if (!tab || tab.binary) return false;
      if (tab.invalidEncoding && !tab.allowLossyConversion) {
        toast("Non-UTF-8 file is read-only. Click 'Convert to UTF-8' in the banner first.", true);
        return false;
      }
      try {
        const res = await rpc("Editor", "saveFile", [
          {
            sessionId,
            path: tab.path,
            content: tab.content,
            lineEnding: tab.lineEnding,
            expectedHash: tab.baseHash,
            force,
          },
        ]);
        if (res?.conflict) {
          tab.conflict = {
            diskContent: res.diskContent,
            diskHash: res.diskHash,
            diskMtimeMs: res.diskMtimeMs,
          };
          paint();
          toast("File changed on disk. Review the conflict banner before saving.", true);
          return false;
        }
        if (res?.error) throw new Error(res.error);
        tab.savedContent = tab.content;
        tab.dirty = false;
        tab.baseHash = res.hash || tab.baseHash;
        tab.mtimeMs = res.mtimeMs || tab.mtimeMs;
        tab.conflict = null;
        await persistNow();
        updateDirtyUi(tab);
        updateStatusBar(tab);
        toast(`Saved ${tab.name}`);
        return true;
      } catch (error) {
        toast(error.message, true);
        return false;
      }
    };

    const updateDirtyUi = (tab) => {
      const activeNode = host?.querySelector(".editor-tab.active");
      if (!activeNode || !tab) return;
      activeNode.classList.toggle("is-dirty", tab.dirty);
      if (tab.dirty && !activeNode.querySelector(".editor-dirty-dot")) {
        activeNode.insertBefore(
          el("span", { class: "editor-dirty-dot", title: "Unsaved changes" }, "●"),
          activeNode.querySelector(".editor-tab-close"),
        );
      } else if (!tab.dirty) {
        activeNode.querySelector(".editor-dirty-dot")?.remove();
      }
    };

    const updateStatusBar = (tab) => {
      const status = host?.querySelector(".editor-status-meta");
      if (status && tab) {
        status.textContent = `${tab.language} · ${tab.encoding} · ${tab.lineEnding} · ${tab.dirty ? "Unsaved" : "Saved"}`;
      }
    };

    const closeTab = async (pathToClose) => {
      const idx = tabs.findIndex((t) => samePath(t.path, pathToClose));
      if (idx < 0) return;
      const tab = tabs[idx];
      const performClose = () => {
        if (tab.monacoModel) {
          try {
            tab.monacoModel.dispose();
          } catch (_) {}
          tab.monacoModel = null;
        }
        tabs.splice(idx, 1);
        if (samePath(activePath, pathToClose)) {
          activePath = tabs[Math.min(idx, tabs.length - 1)]?.path || "";
        }
        schedulePersist();
        paint();
      };
      if (!tab.dirty) {
        performClose();
        return;
      }
      const d = dialog(
        `Save changes to ${tab.name}?`,
        el(
          "div",
          {},
          el("p", { class: "hint" }, "Your manual edits have not been saved yet."),
          el(
            "div",
            { class: "form-actions" },
            button("Cancel", () => d?.close?.(), "quiet"),
            button(
              "Discard",
              () => {
                d?.close?.();
                performClose();
              },
              "danger",
            ),
            button(
              "Save",
              async () => {
                const ok = await saveTab(tab);
                if (ok) {
                  d?.close?.();
                  performClose();
                }
              },
              "primary",
            ),
          ),
        ),
      );
    };

    const openFile = async (opts = {}) => {
      try {
        const existing = opts.path
          ? tabs.find((t) => samePath(t.path, opts.path))
          : opts.sourceId && opts.relativePath !== undefined
            ? tabs.find((t) => t.sourceId === opts.sourceId && t.relativePath === opts.relativePath)
            : null;
        if (existing) {
          activePath = existing.path;
          isOpen = true;
          if (layoutMode === "chat-max") layoutMode = "split";
          notifyLayout();
          schedulePersist();
          paint();
          focusEditor();
          return existing;
        }

        const res = await rpc("Editor", "openFile", [
          {
            sessionId,
            sourceId: opts.sourceId || "",
            relativePath: opts.relativePath || "",
            path: opts.path || "",
          },
        ]);
        if (res?.error && !res.binary && !res.invalidEncoding) throw new Error(res.error);

        const existingByResolved = tabs.find((t) => samePath(t.path, res.path));
        if (existingByResolved) {
          activePath = existingByResolved.path;
          isOpen = true;
          if (layoutMode === "chat-max") layoutMode = "split";
          notifyLayout();
          schedulePersist();
          paint();
          focusEditor();
          return existingByResolved;
        }

        const lang = detectLanguageFromName(res.name || opts.name, res.language);
        const tab = {
          path: res.path,
          name: res.name || opts.name || "untitled",
          relativePath: res.relativePath || opts.relativePath || "",
          sourceId: opts.sourceId || "",
          language: lang,
          content: res.binary ? "" : res.content ?? "",
          savedContent: res.binary ? "" : res.content ?? "",
          dirty: false,
          binary: Boolean(res.binary),
          binaryMessage: res.binary ? res.error : "",
          invalidEncoding: Boolean(res.invalidEncoding),
          encodingWarning: res.invalidEncoding ? res.error : "",
          allowLossyConversion: false,
          readOnly: Boolean(res.readOnly),
          baseHash: res.hash || "",
          mtimeMs: res.mtimeMs || 0,
          lineEnding: res.lineEnding || "LF",
          encoding: res.encoding || "UTF-8",
          scrollTop: 0,
          cursorStart: 0,
          cursorEnd: 0,
          undoStack: [],
          redoStack: [],
          conflict: null,
          monacoModel: null,
        };
        tabs.push(tab);
        activePath = tab.path;
        isOpen = true;
        if (layoutMode === "chat-max") layoutMode = "split";
        notifyLayout();
        schedulePersist();
        paint();
        focusEditor();
        return tab;
      } catch (error) {
        toast(error.message, true);
        return null;
      }
    };

    const focusEditor = () => {
      if (monacoInstance) {
        monacoInstance.focus();
      } else {
        host?.querySelector(".editor-input")?.focus();
      }
    };

    const handleExternalChange = async (payload) => {
      const targetPath = payload?.path;
      const matched = targetPath ? tabs.filter((t) => samePath(t.path, targetPath)) : tabs.slice();
      for (const tab of matched) {
        if (tab.binary) continue;
        try {
          const st = await rpc("Editor", "statFile", [
            { sessionId, path: tab.path, includeContent: true },
          ]);
          if (!st?.exists) continue;
          if (st.hash && st.hash !== tab.baseHash) {
            if (!tab.dirty) {
              tab.content = st.content ?? tab.content;
              tab.savedContent = tab.content;
              tab.baseHash = st.hash;
              tab.mtimeMs = st.mtimeMs || tab.mtimeMs;
              tab.lineEnding = st.lineEnding || tab.lineEnding;
              tab.conflict = null;
              if (tab.monacoModel) {
                suppressModelChange = true;
                tab.monacoModel.setValue(tab.content);
                suppressModelChange = false;
              }
              schedulePersist();
              paint();
              toast(`Reloaded ${tab.name} (updated on disk)`);
            } else {
              tab.conflict = {
                diskContent: st.content ?? "",
                diskHash: st.hash,
                diskMtimeMs: st.mtimeMs || 0,
              };
              paint();
            }
          }
        } catch (_) {}
      }
    };

    const showDiffDialog = (tab) => {
      if (!tab?.conflict) return;
      const container = el("div", { class: "editor-diff-container" });
      if (monacoApi?.editor?.createDiffEditor) {
        const diffEl = el("div", { class: "editor-monaco-diff" });
        container.append(
          el("p", { class: "hint" }, "Left: Disk (AI/external) · Right: Your unsaved local edits"),
          diffEl,
        );
        const d = dialog(`Diff: ${tab.name}`, container, { wide: true });
        const diffEditor = monacoApi.editor.createDiffEditor(diffEl, {
          theme: currentMonacoTheme(),
          readOnly: true,
          automaticLayout: true,
          renderSideBySide: true,
        });
        const origModel = monacoApi.editor.createModel(tab.conflict.diskContent, tab.language);
        const modModel = monacoApi.editor.createModel(tab.content, tab.language);
        diffEditor.setModel({ original: origModel, modified: modModel });
        d?.addEventListener?.("close", () => {
          diffEditor.dispose();
          origModel.dispose();
          modModel.dispose();
        });
      } else {
        const diffText = simpleLineDiff(tab.content, tab.conflict.diskContent);
        dialog(
          `Diff: ${tab.name} (local vs disk)`,
          el(
            "div",
            {},
            el("p", { class: "hint" }, "Lines starting with '-' are local edits; '+' are from disk."),
            el("pre", { class: "code-preview" }, diffText || "(no differences)"),
          ),
          { wide: true },
        );
      }
    };

    const attachMonaco = async (surfaceNode, tab) => {
      if (!monacoApi) monacoApi = await loadMonaco();
      if (!monacoApi || !surfaceNode.isConnected) {
        attachTextareaFallback(surfaceNode, tab);
        return;
      }
      if (!tab.monacoModel) {
        const uri = monacoApi.Uri.file(tab.path);
        const existingModel = monacoApi.editor.getModel(uri);
        if (existingModel) {
          tab.monacoModel = existingModel;
          if (existingModel.getValue() !== tab.content) {
            suppressModelChange = true;
            existingModel.setValue(tab.content);
            suppressModelChange = false;
          }
          monacoApi.editor.setModelLanguage(existingModel, tab.language);
        } else {
          tab.monacoModel = monacoApi.editor.createModel(tab.content, tab.language, uri);
        }
        tab.monacoModel.onDidChangeContent(() => {
          if (suppressModelChange) return;
          tab.content = tab.monacoModel.getValue();
          tab.dirty = tab.content !== tab.savedContent;
          updateDirtyUi(tab);
          updateStatusBar(tab);
          schedulePersist();
        });
      } else {
        monacoApi.editor.setModelLanguage(tab.monacoModel, tab.language);
      }

      if (!monacoInstance || monacoHostNode !== surfaceNode) {
        disposeMonaco();
        monacoHostNode = surfaceNode;
        monacoInstance = monacoApi.editor.create(surfaceNode, {
          model: tab.monacoModel,
          theme: currentMonacoTheme(),
          language: tab.language,
          readOnly: Boolean(tab.readOnly && !tab.allowLossyConversion),
          automaticLayout: true,
          fontSize: 13,
          lineNumbers: "on",
          minimap: { enabled: minimapEnabled },
          wordWrap: wordWrap ? "on" : "off",
          autoIndent: "full",
          autoClosingBrackets: "always",
          autoClosingQuotes: "always",
          multiCursorModifier: "alt",
          scrollBeyondLastLine: false,
          tabSize: 2,
        });
        monacoInstance.addCommand(monacoApi.KeyMod.CtrlCmd | monacoApi.KeyCode.KeyS, () => {
          void saveTab(activeTab());
        });
        monacoInstance.onDidChangeCursorPosition((ev) => {
          const cur = activeTab();
          if (!cur) return;
          const posNode = host?.querySelector(".editor-cursor-pos");
          if (posNode) posNode.textContent = `Ln ${ev.position.lineNumber}, Col ${ev.position.column}`;
          const model = monacoInstance.getModel();
          if (model) {
            cur.cursorStart = model.getOffsetAt(ev.position);
            cur.cursorEnd = cur.cursorStart;
          }
        });
        monacoInstance.onDidScrollChange((ev) => {
          const cur = activeTab();
          if (cur) {
            cur.scrollTop = ev.scrollTop;
            schedulePersist();
          }
        });
      } else {
        monacoInstance.setModel(tab.monacoModel);
        monacoInstance.updateOptions({
          theme: currentMonacoTheme(),
          readOnly: Boolean(tab.readOnly && !tab.allowLossyConversion),
          minimap: { enabled: minimapEnabled },
          wordWrap: wordWrap ? "on" : "off",
        });
      }
      if (tab.scrollTop) monacoInstance.setScrollTop(tab.scrollTop);
      if (tab.cursorStart) {
        const pos = tab.monacoModel.getPositionAt(tab.cursorStart);
        monacoInstance.setPosition(pos);
      }
      const pos = monacoInstance.getPosition();
      const posNode = host?.querySelector(".editor-cursor-pos");
      if (pos && posNode) posNode.textContent = `Ln ${pos.lineNumber}, Col ${pos.column}`;
    };

    const attachTextareaFallback = (surfaceNode, tab) => {
      const ta = el("textarea", {
        class: "editor-input editor-fallback-input",
        spellcheck: "false",
        readOnly: Boolean(tab.readOnly && !tab.allowLossyConversion),
        "aria-label": `Editing ${tab.name}`,
        onInput: (e) => {
          tab.content = e.target.value;
          tab.dirty = tab.content !== tab.savedContent;
          tab.cursorStart = e.target.selectionStart || 0;
          tab.cursorEnd = e.target.selectionEnd || 0;
          updateDirtyUi(tab);
          updateStatusBar(tab);
          schedulePersist();
        },
        onKeydown: (e) => {
          if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
            e.preventDefault();
            void saveTab(tab);
          }
        },
      });
      ta.value = tab.content;
      surfaceNode.replaceChildren(ta);
    };

    const paint = () => {
      if (!host) return;
      host.hidden = !isOpen || layoutMode === "chat-max";
      if (!isOpen) {
        disposeMonaco();
        host.replaceChildren();
        return;
      }

      const tab = activeTab();
      const tabNodes = tabs.map((t) => {
        const isActive = t.path === tab?.path;
        return el(
          "div",
          {
            class: "editor-tab" + (isActive ? " active" : "") + (t.dirty ? " is-dirty" : ""),
            role: "tab",
            "aria-selected": String(isActive),
            draggable: "true",
            title: t.path,
            onClick: () => {
              activePath = t.path;
              schedulePersist();
              paint();
            },
            onDragstart: () => {
              dragPath = t.path;
            },
            onDragover: (e) => e.preventDefault(),
            onDrop: (e) => {
              e.preventDefault();
              if (!dragPath || dragPath === t.path) return;
              const fromIdx = tabs.findIndex((x) => x.path === dragPath);
              const toIdx = tabs.findIndex((x) => x.path === t.path);
              if (fromIdx >= 0 && toIdx >= 0) {
                const [moved] = tabs.splice(fromIdx, 1);
                tabs.splice(toIdx, 0, moved);
                dragPath = null;
                schedulePersist();
                paint();
              }
            },
          },
          el("span", { class: "editor-tab-name" }, t.name),
          t.dirty ? el("span", { class: "editor-dirty-dot", title: "Unsaved changes" }, "●") : null,
          el(
            "button",
            {
              type: "button",
              class: "editor-tab-close",
              "aria-label": `Close ${t.name}`,
              onClick: (e) => {
                e.stopPropagation();
                void closeTab(t.path);
              },
            },
            "×",
          ),
        );
      });

      const toolbar = el(
        "div",
        { class: "editor-toolbar" },
        el("div", { class: "editor-tabs", role: "tablist" }, ...tabNodes),
        el(
          "div",
          { class: "editor-actions" },
          button("Save", () => void saveTab(), "quiet", "check"),
          ib("Find & Replace", "search", () => {
            if (monacoInstance) {
              monacoInstance.focus();
              monacoInstance.trigger("toolbar", "editor.action.startFindReplaceAction", null);
            }
          }),
          button(
            wordWrap ? "Wrap: On" : "Wrap: Off",
            () => {
              wordWrap = !wordWrap;
              paint();
            },
            "quiet",
          ),
          button(
            minimapEnabled ? "Map: On" : "Map: Off",
            () => {
              minimapEnabled = !minimapEnabled;
              paint();
            },
            "quiet",
          ),
          button(
            layoutMode === "editor-max" ? "Split" : "Max Editor",
            () => {
              layoutMode = layoutMode === "editor-max" ? "split" : "editor-max";
              notifyLayout();
              schedulePersist();
              paint();
            },
            "quiet",
          ),
          button(
            "Max Chat",
            () => {
              layoutMode = "chat-max";
              notifyLayout();
              schedulePersist();
              paint();
            },
            "quiet",
          ),
          ib("Close Editor", "close", () => {
            isOpen = false;
            notifyLayout();
            schedulePersist();
            paint();
          }),
        ),
      );

      const encodingBanner = tab?.invalidEncoding && !tab.allowLossyConversion
        ? el(
            "div",
            { class: "editor-conflict-banner", role: "alert" },
            el("span", {}, tab.encodingWarning || "Non-UTF-8 file opened read-only."),
            button(
              "Convert to UTF-8",
              () => {
                tab.allowLossyConversion = true;
                tab.readOnly = false;
                tab.encoding = "UTF-8 (converted)";
                paint();
              },
              "primary",
            ),
          )
        : null;

      const conflictBanner = tab?.conflict
        ? el(
            "div",
            { class: "editor-conflict-banner", role: "alert" },
            el("span", {}, "Modified externally on disk (e.g. by AI) while you have unsaved edits."),
            el(
              "div",
              { class: "row" },
              button("Compare diff", () => showDiffDialog(tab), "quiet"),
              button(
                "Reload disk",
                () => {
                  tab.content = tab.conflict.diskContent;
                  tab.savedContent = tab.content;
                  tab.baseHash = tab.conflict.diskHash;
                  tab.mtimeMs = tab.conflict.diskMtimeMs;
                  tab.dirty = false;
                  tab.conflict = null;
                  if (tab.monacoModel) {
                    suppressModelChange = true;
                    tab.monacoModel.setValue(tab.content);
                    suppressModelChange = false;
                  }
                  schedulePersist();
                  paint();
                },
                "quiet",
              ),
              button(
                "Keep local",
                () => {
                  tab.baseHash = tab.conflict.diskHash;
                  tab.conflict = null;
                  paint();
                },
                "primary",
              ),
            ),
          )
        : null;

      let body;
      if (!tab) {
        disposeMonaco();
        body = el(
          "div",
          { class: "editor-empty" },
          icon("file", 28),
          el("strong", {}, "No file open"),
          el("p", { class: "hint" }, "Double-click any file in the Explorer to edit it here."),
        );
      } else if (tab.binary) {
        disposeMonaco();
        body = el(
          "div",
          { class: "editor-empty" },
          icon("file", 28),
          el("strong", {}, tab.name),
          el("p", { class: "error-text" }, tab.binaryMessage || "Binary file cannot be edited as text."),
        );
      } else {
        body = el("div", { class: "editor-surface editor-monaco-host" });
      }

      const status = el(
        "div",
        { class: "editor-statusbar" },
        el("span", { class: "editor-cursor-pos" }, "Ln 1, Col 1"),
        tab
          ? el(
              "span",
              { class: "editor-status-meta hint" },
              `${tab.language} · ${tab.encoding} · ${tab.lineEnding} · ${tab.dirty ? "Unsaved" : "Saved"}`,
            )
          : el("span", { class: "editor-status-meta hint" }, "Ready"),
      );

      // Preserve live Monaco host node if switching tabs without destroying editor instance
      const existingShell = host.querySelector(".editor-shell");
      const existingMonacoHost = existingShell?.querySelector(".editor-monaco-host");
      if (tab && !tab.binary && existingShell && existingMonacoHost && monacoInstance && monacoHostNode === existingMonacoHost) {
        existingShell.querySelector(".editor-toolbar")?.replaceWith(toolbar);
        existingShell.querySelectorAll(".editor-conflict-banner").forEach((n) => n.remove());
        if (encodingBanner) existingMonacoHost.before(encodingBanner);
        if (conflictBanner) existingMonacoHost.before(conflictBanner);
        existingShell.querySelector(".editor-statusbar")?.replaceWith(status);
        void attachMonaco(existingMonacoHost, tab);
      } else {
        host.replaceChildren(
          el("div", { class: "editor-shell" }, toolbar, encodingBanner, conflictBanner, body, status),
        );
        if (tab && !tab.binary) {
          void attachMonaco(body, tab);
        }
      }
    };

    const startPoll = () => {
      clearInterval(pollTimer);
      pollTimer = setInterval(() => {
        if (!isOpen || !tabs.length || document.hidden) return;
        for (const t of tabs) void handleExternalChange({ path: t.path });
      }, STAT_POLL_MS);
    };

    const loadWorkspace = async (nextSessionId, nextWorkspaceKey) => {
      sessionId = nextSessionId || sessionId;
      const targetKey = nextWorkspaceKey || "default";
      if (targetKey === workspaceKey && tabs.length) {
        paint();
        return;
      }
      workspaceKey = targetKey;
      try {
        const saved = await rpc("Editor", "getWorkspaceState", [{ workspaceKey }]);
        if (!saved || saved.error) return;
        isOpen = Boolean(saved.isOpen);
        layoutMode = saved.layoutMode || "split";
        widthPx = Number(saved.widthPx) || 560;
        const restored = [];
        for (const t of saved.tabs || []) {
          if (!t.path) continue;
          const disk = await rpc("Editor", "openFile", [{ sessionId, path: t.path }]);
          const diskContent = disk && !disk.error && !disk.binary ? disk.content : "";
          const useDraft = Boolean(t.dirty && typeof t.draftContent === "string");
          restored.push({
            path: t.path,
            name: t.name || disk?.name || "file",
            relativePath: t.relativePath || "",
            sourceId: t.sourceId || "",
            language: detectLanguageFromName(t.name || disk?.name, t.language || disk?.language),
            content: useDraft ? t.draftContent : diskContent,
            savedContent: diskContent,
            dirty: useDraft && t.draftContent !== diskContent,
            binary: Boolean(disk?.binary),
            binaryMessage: disk?.binary ? disk.error : "",
            invalidEncoding: Boolean(disk?.invalidEncoding),
            encodingWarning: disk?.invalidEncoding ? disk.error : "",
            allowLossyConversion: false,
            readOnly: Boolean(disk?.readOnly),
            baseHash: t.baseHash || disk?.hash || "",
            mtimeMs: disk?.mtimeMs || 0,
            lineEnding: t.lineEnding || disk?.lineEnding || "LF",
            encoding: disk?.encoding || "UTF-8",
            scrollTop: Number(t.scrollTop) || 0,
            cursorStart: Number(t.cursorStart) || 0,
            cursorEnd: Number(t.cursorEnd) || 0,
            undoStack: [],
            redoStack: [],
            conflict: null,
            monacoModel: null,
          });
        }
        tabs = restored;
        activePath = saved.activePath || restored[0]?.path || "";
        notifyLayout();
        paint();
      } catch (_) {}
    };

    return {
      mount(target) {
        host = target;
        startPoll();
        paint();
      },
      openFile,
      saveTab,
      closeTab,
      loadWorkspace,
      handleExternalChange,
      setSession(sid) {
        sessionId = sid || "";
      },
      setOpen(open) {
        isOpen = Boolean(open);
        if (isOpen && layoutMode === "chat-max") layoutMode = "split";
        notifyLayout();
        schedulePersist();
        paint();
      },
      setWidth(w) {
        widthPx = Math.max(320, Math.min(1200, Number(w) || 560));
        notifyLayout();
        schedulePersist();
      },
      setLayoutMode(mode) {
        layoutMode = mode;
        notifyLayout();
        schedulePersist();
        paint();
      },
      getState() {
        return { isOpen, layoutMode, widthPx, activePath, tabs };
      },
      dispose() {
        clearTimeout(saveTimer);
        clearInterval(pollTimer);
        disposeMonaco();
        for (const t of tabs) {
          try {
            t.monacoModel?.dispose();
          } catch (_) {}
        }
      },
    };
  }

  window.TsEditorPanel = { create, detectLanguageFromName };
})();
