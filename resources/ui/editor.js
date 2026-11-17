/* Copyright (c) 2026 QuixiAI. MIT license. Integrated tabbed code/text editor
   with syntax highlighting, undo/redo, find/replace, Ctrl+S atomic save,
   external AI change conflict handling, and per-workspace layout persistence. */
(() => {
  "use strict";

  const TEXT_TAB = "  ";
  const SAVE_DEBOUNCE_MS = 350;
  const STAT_POLL_MS = 2500;

  function escapeHtml(text) {
    return String(text || "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function highlightLine(line, lang) {
    const escaped = escapeHtml(line);
    if (!escaped) return " ";
    if (lang === "plaintext") return escaped;

    const rules = [];
    if (lang === "json") {
      rules.push([/"(?:\\.|[^"\\])*"(?=\s*:)/g, "tok-key"]);
      rules.push([/"(?:\\.|[^"\\])*"/g, "tok-str"]);
      rules.push([/\b(?:true|false|null)\b/g, "tok-kw"]);
      rules.push([/-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b/g, "tok-num"]);
    } else if (lang === "yaml" || lang === "toml") {
      rules.push([/#.*$/g, "tok-com"]);
      rules.push([/^(\s*[A-Za-z0-9_.-]+)(?=\s*[:=])/gm, "tok-key"]);
      rules.push([/"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/g, "tok-str"]);
      rules.push([/\b(?:true|false|null)\b/g, "tok-kw"]);
      rules.push([/\b\d+(?:\.\d+)?\b/g, "tok-num"]);
    } else if (lang === "markdown") {
      rules.push([/^#{1,6}\s+.*$/gm, "tok-kw"]);
      rules.push([/`[^`]+`/g, "tok-str"]);
      rules.push([/\*\*[^*]+\*\*/g, "tok-key"]);
    } else if (lang === "html" || lang === "xml") {
      rules.push([/&lt;!--[\s\S]*?--&gt;/g, "tok-com"]);
      rules.push([/&lt;\/?[A-Za-z0-9_-]+/g, "tok-kw"]);
      rules.push([/"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/g, "tok-str"]);
    } else if (lang === "css") {
      rules.push([/\/\*[\s\S]*?\*\//g, "tok-com"]);
      rules.push([/[A-Za-z-]+(?=\s*:)/g, "tok-key"]);
      rules.push([/"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/g, "tok-str"]);
      rules.push([/#[0-9a-fA-F]{3,8}\b|\b\d+(?:\.\d+)?(?:px|rem|em|%|vh|vw|ms|s)?\b/g, "tok-num"]);
    } else if (lang === "python" || lang === "shell") {
      rules.push([/#.*$/gm, "tok-com"]);
      rules.push([/"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'/g, "tok-str"]);
      rules.push([/\b(?:def|class|import|from|as|if|elif|else|for|while|return|try|except|finally|with|async|await|yield|pass|break|continue|True|False|None|in|is|not|and|or|lambda)\b/g, "tok-kw"]);
      rules.push([/\b\d+(?:\.\d+)?\b/g, "tok-num"]);
    } else {
      // rust, javascript, typescript, php, etc.
      rules.push([/\/\/.*$/gm, "tok-com"]);
      rules.push([/\/\*[\s\S]*?\*\//g, "tok-com"]);
      rules.push([/"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`/g, "tok-str"]);
      rules.push([/\b(?:fn|let|mut|const|pub|mod|use|struct|enum|impl|trait|type|where|match|if|else|for|while|loop|return|async|await|function|var|class|import|export|from|default|try|catch|finally|throw|new|true|false|null|undefined|Some|None|Ok|Err|self|Self)\b/g, "tok-kw"]);
      rules.push([/\b\d+(?:\.\d+)?\b/g, "tok-num"]);
    }

    // Tokenize without overlapping spans using a single-pass scanner on placeholders
    const tokens = [];
    let work = escaped;
    for (const [regex, cls] of rules) {
      work = work.replace(regex, (m) => {
        if (m.includes("\u0001")) return m;
        const idx = tokens.length;
        tokens.push(`<span class="${cls}">${m}</span>`);
        return `\u0001${idx}\u0002`;
      });
    }
    return work.replace(/\u0001(\d+)\u0002/g, (_, idx) => tokens[Number(idx)] || "");
  }

  function highlightCode(text, lang) {
    return String(text || "")
      .split("\n")
      .map((line) => highlightLine(line, lang))
      .join("\n");
  }

  function simpleLineDiff(localText, diskText) {
    const a = String(localText || "").split("\n");
    const b = String(diskText || "").split("\n");
    const max = Math.max(a.length, b.length);
    const out = [];
    for (let i = 0; i < max; i += 1) {
      const la = a[i];
      const lb = b[i];
      if (la === lb) {
        out.push(`  ${la ?? ""}`);
      } else {
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
    let layoutMode = "split"; // "split" | "editor-max" | "chat-max"
    let widthPx = 520;
    let tabs = [];
    let activePath = "";
    let findOpen = false;
    let findQuery = "";
    let replaceQuery = "";
    let findMatches = [];
    let findIndex = 0;
    let saveTimer = null;
    let pollTimer = null;
    let dragPath = null;
    let suppressInput = false;

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

    const pushUndo = (tab, nextContent) => {
      if (tab.content === nextContent) return;
      tab.undoStack = tab.undoStack || [];
      tab.redoStack = tab.redoStack || [];
      tab.undoStack.push({
        content: tab.content,
        cursorStart: tab.cursorStart,
        cursorEnd: tab.cursorEnd,
      });
      if (tab.undoStack.length > 100) tab.undoStack.shift();
      tab.redoStack = [];
    };

    const applyEdit = (tab, nextContent, cursorStart, cursorEnd) => {
      pushUndo(tab, nextContent);
      tab.content = nextContent;
      tab.dirty = tab.content !== tab.savedContent;
      if (typeof cursorStart === "number") tab.cursorStart = cursorStart;
      if (typeof cursorEnd === "number") tab.cursorEnd = cursorEnd;
      schedulePersist();
    };

    const undo = () => {
      const tab = activeTab();
      if (!tab || !tab.undoStack?.length) return;
      tab.redoStack = tab.redoStack || [];
      tab.redoStack.push({
        content: tab.content,
        cursorStart: tab.cursorStart,
        cursorEnd: tab.cursorEnd,
      });
      const prev = tab.undoStack.pop();
      tab.content = prev.content;
      tab.cursorStart = prev.cursorStart;
      tab.cursorEnd = prev.cursorEnd;
      tab.dirty = tab.content !== tab.savedContent;
      schedulePersist();
      paint();
    };

    const redo = () => {
      const tab = activeTab();
      if (!tab || !tab.redoStack?.length) return;
      tab.undoStack = tab.undoStack || [];
      tab.undoStack.push({
        content: tab.content,
        cursorStart: tab.cursorStart,
        cursorEnd: tab.cursorEnd,
      });
      const next = tab.redoStack.pop();
      tab.content = next.content;
      tab.cursorStart = next.cursorStart;
      tab.cursorEnd = next.cursorEnd;
      tab.dirty = tab.content !== tab.savedContent;
      schedulePersist();
      paint();
    };

    const saveTab = async (tab = activeTab(), force = false) => {
      if (!tab || tab.binary) return false;
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
        paint();
        toast(`Saved ${tab.name}`);
        return true;
      } catch (error) {
        toast(error.message, true);
        return false;
      }
    };

    const closeTab = async (pathToClose) => {
      const idx = tabs.findIndex((t) => samePath(t.path, pathToClose));
      if (idx < 0) return;
      const tab = tabs[idx];
      const performClose = () => {
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
            ? tabs.find(
                (t) =>
                  t.sourceId === opts.sourceId &&
                  t.relativePath === opts.relativePath,
              )
            : null;
        if (existing) {
          activePath = existing.path;
          isOpen = true;
          if (layoutMode === "chat-max") layoutMode = "split";
          notifyLayout();
          schedulePersist();
          paint();
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
        if (res?.error && !res.binary) throw new Error(res.error);

        const existingByResolved = tabs.find((t) => samePath(t.path, res.path));
        if (existingByResolved) {
          activePath = existingByResolved.path;
          isOpen = true;
          if (layoutMode === "chat-max") layoutMode = "split";
          notifyLayout();
          schedulePersist();
          paint();
          return existingByResolved;
        }

        const tab = {
          path: res.path,
          name: res.name || opts.name || "untitled",
          relativePath: res.relativePath || opts.relativePath || "",
          sourceId: opts.sourceId || "",
          language: res.language || "plaintext",
          content: res.binary ? "" : res.content ?? "",
          savedContent: res.binary ? "" : res.content ?? "",
          dirty: false,
          binary: Boolean(res.binary),
          binaryMessage: res.binary ? res.error : "",
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
        };
        tabs.push(tab);
        activePath = tab.path;
        isOpen = true;
        if (layoutMode === "chat-max") layoutMode = "split";
        notifyLayout();
        schedulePersist();
        paint();
        return tab;
      } catch (error) {
        toast(error.message, true);
        return null;
      }
    };

    const handleExternalChange = async (payload) => {
      const targetPath = payload?.path;
      const matched = targetPath
        ? tabs.filter((t) => samePath(t.path, targetPath))
        : tabs.slice();
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

    const recomputeFind = (tab) => {
      findMatches = [];
      if (!tab || !findQuery) {
        findIndex = 0;
        return;
      }
      const hay = tab.content.toLowerCase();
      const needle = findQuery.toLowerCase();
      let at = 0;
      while (at < hay.length && findMatches.length < 1000) {
        const pos = hay.indexOf(needle, at);
        if (pos < 0) break;
        findMatches.push(pos);
        at = pos + Math.max(1, needle.length);
      }
      if (findIndex >= findMatches.length) findIndex = 0;
    };

    const jumpFind = (delta) => {
      const tab = activeTab();
      if (!tab || !findMatches.length) return;
      findIndex = (findIndex + delta + findMatches.length) % findMatches.length;
      const start = findMatches[findIndex];
      const end = start + findQuery.length;
      tab.cursorStart = start;
      tab.cursorEnd = end;
      paint();
      const ta = host?.querySelector(".editor-input");
      if (ta) {
        ta.focus();
        ta.setSelectionRange(start, end);
      }
    };

    const replaceOne = () => {
      const tab = activeTab();
      if (!tab || !findMatches.length) return;
      const start = findMatches[findIndex];
      const end = start + findQuery.length;
      const next = tab.content.slice(0, start) + replaceQuery + tab.content.slice(end);
      applyEdit(tab, next, start, start + replaceQuery.length);
      recomputeFind(tab);
      paint();
    };

    const replaceAll = () => {
      const tab = activeTab();
      if (!tab || !findQuery) return;
      const escaped = findQuery.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      const next = tab.content.replace(new RegExp(escaped, "gi"), replaceQuery);
      applyEdit(tab, next, 0, 0);
      recomputeFind(tab);
      paint();
    };

    const showDiffDialog = (tab) => {
      if (!tab?.conflict) return;
      const diffText = simpleLineDiff(tab.content, tab.conflict.diskContent);
      dialog(
        `Diff: ${tab.name} (local vs disk)`,
        el(
          "div",
          {},
          el(
            "p",
            { class: "hint" },
            "Lines starting with '-' are your unsaved local edits; lines starting with '+' are from disk.",
          ),
          el("pre", { class: "code-preview" }, diffText || "(no differences)"),
        ),
        { wide: true },
      );
    };

    const updateLineCol = (tab, ta) => {
      const pos = ta?.selectionStart ?? tab.cursorStart ?? 0;
      const before = tab.content.slice(0, pos);
      const lines = before.split("\n");
      const line = lines.length;
      const col = lines[lines.length - 1].length + 1;
      const badge = host?.querySelector(".editor-cursor-pos");
      if (badge) badge.textContent = `Ln ${line}, Col ${col}`;
    };

    const paint = () => {
      if (!host) return;
      host.hidden = !isOpen || layoutMode === "chat-max";
      if (!isOpen) {
        host.replaceChildren();
        return;
      }

      const tab = activeTab();
      recomputeFind(tab);

      const tabNodes = tabs.map((t) => {
        const isActive = t.path === tab?.path;
        const tabEl = el(
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
        return tabEl;
      });

      const toolbar = el(
        "div",
        { class: "editor-toolbar" },
        el("div", { class: "editor-tabs", role: "tablist" }, ...tabNodes),
        el(
          "div",
          { class: "editor-actions" },
          button("Save", () => void saveTab(), "quiet", "check"),
          ib("Undo", "back", undo),
          ib("Redo", "arrow", redo),
          ib("Find & Replace", "search", () => {
            findOpen = !findOpen;
            paint();
          }),
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

      const findBar = findOpen
        ? el(
            "div",
            { class: "editor-find-bar" },
            el("input", {
              type: "search",
              placeholder: "Find…",
              value: findQuery,
              onInput: (e) => {
                findQuery = e.target.value;
                recomputeFind(tab);
                const cnt = host?.querySelector(".editor-find-count");
                if (cnt) cnt.textContent = findMatches.length ? `${findIndex + 1}/${findMatches.length}` : "0/0";
              },
              onKeydown: (e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  jumpFind(e.shiftKey ? -1 : 1);
                } else if (e.key === "Escape") {
                  findOpen = false;
                  paint();
                }
              },
            }),
            el("input", {
              type: "text",
              placeholder: "Replace…",
              value: replaceQuery,
              onInput: (e) => {
                replaceQuery = e.target.value;
              },
            }),
            el(
              "span",
              { class: "editor-find-count hint" },
              findMatches.length ? `${findIndex + 1}/${findMatches.length}` : "0/0",
            ),
            button("Prev", () => jumpFind(-1), "quiet"),
            button("Next", () => jumpFind(1), "quiet"),
            button("Replace", replaceOne, "quiet"),
            button("All", replaceAll, "quiet"),
            ib("Close find", "close", () => {
              findOpen = false;
              paint();
            }),
          )
        : null;

      const conflictBanner = tab?.conflict
        ? el(
            "div",
            { class: "editor-conflict-banner", role: "alert" },
            el(
              "span",
              {},
              "Modified externally on disk (e.g. by AI) while you have unsaved edits.",
            ),
            el(
              "div",
              { class: "row" },
              button("Compare diff", () => showDiffDialog(tab), "quiet"),
              button(
                "Reload disk",
                () => {
                  pushUndo(tab, tab.conflict.diskContent);
                  tab.content = tab.conflict.diskContent;
                  tab.savedContent = tab.content;
                  tab.baseHash = tab.conflict.diskHash;
                  tab.mtimeMs = tab.conflict.diskMtimeMs;
                  tab.dirty = false;
                  tab.conflict = null;
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
        body = el(
          "div",
          { class: "editor-empty" },
          icon("file", 28),
          el("strong", {}, "No file open"),
          el(
            "p",
            { class: "hint" },
            "Double-click any file in the Explorer to edit it here alongside your AI conversation.",
          ),
        );
      } else if (tab.binary) {
        body = el(
          "div",
          { class: "editor-empty" },
          icon("file", 28),
          el("strong", {}, tab.name),
          el("p", { class: "error-text" }, tab.binaryMessage || "Binary file cannot be edited as text."),
        );
      } else {
        const lineCount = Math.max(1, tab.content.split("\n").length);
        const gutter = el(
          "div",
          { class: "editor-gutter", "aria-hidden": "true" },
          Array.from({ length: lineCount }, (_, i) => String(i + 1)).join("\n"),
        );
        const highlight = el("pre", { class: "editor-highlight", "aria-hidden": "true" });
        highlight.innerHTML = highlightCode(tab.content, tab.language);
        const ta = el("textarea", {
          class: "editor-input",
          spellcheck: "false",
          wrap: "off",
          "aria-label": `Editing ${tab.name}`,
          onInput: (e) => {
            if (suppressInput) return;
            const next = e.target.value;
            tab.cursorStart = e.target.selectionStart || 0;
            tab.cursorEnd = e.target.selectionEnd || 0;
            applyEdit(tab, next, tab.cursorStart, tab.cursorEnd);
            highlight.innerHTML = highlightCode(next, tab.language);
            const lc = Math.max(1, next.split("\n").length);
            gutter.textContent = Array.from({ length: lc }, (_, i) => String(i + 1)).join("\n");
            const activeNode = host?.querySelector(".editor-tab.active");
            if (activeNode) {
              activeNode.classList.toggle("is-dirty", tab.dirty);
              if (tab.dirty && !activeNode.querySelector(".editor-dirty-dot")) {
                activeNode.insertBefore(
                  el("span", { class: "editor-dirty-dot", title: "Unsaved changes" }, "●"),
                  activeNode.querySelector(".editor-tab-close"),
                );
              } else if (!tab.dirty) {
                activeNode.querySelector(".editor-dirty-dot")?.remove();
              }
            }
            updateLineCol(tab, e.target);
          },
          onScroll: (e) => {
            tab.scrollTop = e.target.scrollTop;
            gutter.scrollTop = e.target.scrollTop;
            highlight.scrollTop = e.target.scrollTop;
            highlight.scrollLeft = e.target.scrollLeft;
            schedulePersist();
          },
          onClick: (e) => {
            tab.cursorStart = e.target.selectionStart || 0;
            tab.cursorEnd = e.target.selectionEnd || 0;
            updateLineCol(tab, e.target);
            schedulePersist();
          },
          onKeyup: (e) => {
            tab.cursorStart = e.target.selectionStart || 0;
            tab.cursorEnd = e.target.selectionEnd || 0;
            updateLineCol(tab, e.target);
          },
          onKeydown: (e) => {
            const mod = e.ctrlKey || e.metaKey;
            if (mod && e.key.toLowerCase() === "s") {
              e.preventDefault();
              void saveTab(tab);
              return;
            }
            if (mod && e.key.toLowerCase() === "f") {
              e.preventDefault();
              findOpen = true;
              paint();
              return;
            }
            if (mod && e.key.toLowerCase() === "z") {
              e.preventDefault();
              if (e.shiftKey) redo();
              else undo();
              return;
            }
            if (mod && e.key.toLowerCase() === "y") {
              e.preventDefault();
              redo();
              return;
            }
            if (e.key === "Tab") {
              e.preventDefault();
              const start = e.target.selectionStart;
              const end = e.target.selectionEnd;
              const next = tab.content.slice(0, start) + TEXT_TAB + tab.content.slice(end);
              suppressInput = true;
              e.target.value = next;
              e.target.setSelectionRange(start + TEXT_TAB.length, start + TEXT_TAB.length);
              suppressInput = false;
              applyEdit(tab, next, start + TEXT_TAB.length, start + TEXT_TAB.length);
              highlight.innerHTML = highlightCode(next, tab.language);
            }
          },
        });
        ta.value = tab.content;
        queueMicrotask(() => {
          ta.scrollTop = tab.scrollTop || 0;
          gutter.scrollTop = tab.scrollTop || 0;
          highlight.scrollTop = tab.scrollTop || 0;
          try {
            ta.setSelectionRange(tab.cursorStart || 0, tab.cursorEnd || 0);
          } catch (_) {}
          updateLineCol(tab, ta);
        });
        body = el(
          "div",
          { class: "editor-surface" },
          gutter,
          el("div", { class: "editor-code-stack" }, highlight, ta),
        );
      }

      const status = el(
        "div",
        { class: "editor-statusbar" },
        el("span", { class: "editor-cursor-pos" }, "Ln 1, Col 1"),
        tab
          ? el(
              "span",
              { class: "hint" },
              `${tab.language} · ${tab.encoding} · ${tab.lineEnding} · ${tab.dirty ? "Unsaved" : "Saved"}`,
            )
          : el("span", { class: "hint" }, "Ready"),
      );

      host.replaceChildren(
        el("div", { class: "editor-shell" }, toolbar, findBar, conflictBanner, body, status),
      );
    };

    const startPoll = () => {
      clearInterval(pollTimer);
      pollTimer = setInterval(() => {
        if (!isOpen || !tabs.length || document.hidden) return;
        for (const t of tabs) {
          void handleExternalChange({ path: t.path });
        }
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
        widthPx = Number(saved.widthPx) || 520;
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
            language: t.language || disk?.language || "plaintext",
            content: useDraft ? t.draftContent : diskContent,
            savedContent: diskContent,
            dirty: useDraft && t.draftContent !== diskContent,
            binary: Boolean(disk?.binary),
            binaryMessage: disk?.binary ? disk.error : "",
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
        widthPx = Math.max(300, Math.min(1200, Number(w) || 520));
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
      },
    };
  }

  window.TsEditorPanel = { create, highlightCode };
})();
