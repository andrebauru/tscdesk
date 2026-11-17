/* Copyright (c) 2026 QuixiAI. MIT license. Explorer panel: independent sources,
   explicit context selection, lazy tree, previews and per-conversation isolation. */
(() => {
  "use strict";

  const SIZE_UNITS = ["B", "KB", "MB", "GB"];

  function cleanPath(value) {
    return String(value || "").replace(/^\\\\\?\\/, "");
  }

  function extBadge(name, isDir) {
    if (isDir) return null;
    const base = String(name || "");
    const dot = base.lastIndexOf(".");
    if (dot <= 0 || dot === base.length - 1) return null;
    const ext = base.slice(dot + 1).slice(0, 5).toUpperCase();
    return ext ? ext : null;
  }

  function formatBytes(bytes) {
    let value = Number(bytes) || 0;
    let unit = 0;
    while (value >= 1024 && unit < SIZE_UNITS.length - 1) {
      value /= 1024;
      unit += 1;
    }
    return unit === 0
      ? `${value} ${SIZE_UNITS[unit]}`
      : `${value.toFixed(value >= 10 ? 0 : 1)} ${SIZE_UNITS[unit]}`;
  }

  function create(deps) {
    const { el, button, ib, icon, toast, rpc, dialog, onOpenFile } = deps;
    let session = null;
    let data = null;
    let preview = null;
    let host = null;
    let projectName = "No project";
    let workspaceFolders = [];
    let openFileHandler = typeof onOpenFile === "function" ? onOpenFile : null;
    let compactMode = false;
    let timer = null;
    let menu = null;

    const call = async (member, body = {}) => {
      if (!session?.sessionId) throw new Error("Open a conversation first.");
      const result = await rpc("Explorer", member, [
        { sessionId: session.sessionId, ...body },
      ]);
      if (result?.error) throw new Error(result.error);
      return result;
    };

    const closeMenu = () => {
      menu?.remove();
      menu = null;
    };

    const showMenu = (event, items) => {
      event.preventDefault();
      event.stopPropagation();
      closeMenu();
      menu = el(
        "div",
        { class: "explorer-menu", role: "menu" },
        ...items.map(([label, action, iconName]) =>
          button(label, async () => {
            closeMenu();
            await action();
          }, "quiet explorer-menu-item", iconName),
        ),
      );
      document.body.append(menu);
      const box = menu.getBoundingClientRect();
      menu.style.left = `${Math.min(event.clientX, innerWidth - box.width - 8)}px`;
      menu.style.top = `${Math.min(event.clientY, innerHeight - box.height - 8)}px`;
      setTimeout(
        () => document.addEventListener("click", closeMenu, { once: true }),
        0,
      );
    };

    const checkbox = (checked, partial, onChange) => {
      const input = el("input", {
        type: "checkbox",
        class: "explorer-check",
        "aria-label": "Select for context",
      });
      input.checked = Boolean(checked);
      input.indeterminate = Boolean(partial);
      input.addEventListener("change", () => onChange(input.checked));
      input.addEventListener("click", (event) => event.stopPropagation());
      return input;
    };

    const refreshPreview = async () => {
      try {
        preview = await call("contextPreview", { windowTokens: 0 });
      } catch (_) {
        preview = null;
      }
    };

    const applyState = async (result) => {
      if (result?.error) throw new Error(result.error);
      data = result;
      await refreshPreview();
      paint();
    };

    const mutate = async (member, body) => {
      try {
        await applyState(await call(member, body));
      } catch (error) {
        toast(error.message, true);
      }
    };

    const previewFile = async (source, relativePath, name) => {
      try {
        const result = await call("previewFile", {
          sourceId: source.id,
          relativePath: relativePath || "",
        });
        if (result?.error && !result.binary) throw new Error(result.error);
        const meta = [
          result.encoding,
          formatBytes(result.bytesRead || 0),
          result.truncated ? "truncated" : null,
          result.binary ? "binary" : null,
        ]
          .filter(Boolean)
          .join(" · ");
        dialog(
          name || source.label,
          el(
            "div",
            {},
            el("p", { class: "hint" }, meta),
            result.binary
              ? el("p", { class: "error-text" }, result.error || "Binary file.")
              : el("pre", { class: "code-preview" }, result.content ?? result.error ?? ""),
          ),
          { wide: true },
        );
      } catch (error) {
        toast(error.message, true);
      }
    };

    const renderRow = (source, entry, depth) => {
      const isDir = entry.isDir;
      const children = el("div", { class: "explorer-children", hidden: true });
      const caret = isDir
        ? button(
            "",
            () => {
              const expanded = children.hidden;
              children.hidden = !expanded;
              caret.classList.toggle("is-open", expanded);
              if (expanded && !children.dataset.loaded) void loadChildren(source, entry.relativePath, children);
              // Persist the tree state without repainting or reloading siblings.
              void call("setExpanded", {
                sourceId: source.id,
                relativePath: entry.relativePath,
                expanded,
              }).catch(() => {});
            },
            "quiet explorer-caret",
            "arrow",
          )
        : el("span", { class: "explorer-caret-spacer" });
      if (!isDir) caret.setAttribute("aria-hidden", "true");

      const name = el(
        "span",
        { class: "explorer-name", title: entry.relativePath },
        entry.name,
      );
      const ext = extBadge(entry.name, isDir);
      const extPill = ext
        ? el("span", { class: "explorer-ext-badge" }, ext)
        : null;
      const ctxPill = entry.checked
        ? el(
            "span",
            {
              class: "explorer-ctx-badge",
              title: "Included in AI context",
            },
            "CTX",
          )
        : null;
      const row = el(
        "div",
        {
          class:
            "explorer-row" +
            (isDir ? " is-dir" : "") +
            (entry.checked ? " is-in-context" : "") +
            (entry.partial ? " is-partial-context" : ""),
          style: `--depth:${depth}`,
          onContextmenu: (event) =>
            showMenu(event, [
              isDir
                ? [
                    "Select folder contents",
                    () =>
                      mutate("setDirectorySelected", {
                        sourceId: source.id,
                        relativePath: entry.relativePath,
                        selected: true,
                      }),
                    "check",
                  ]
                : [
                    openFileHandler ? "Open in Editor" : "Preview file",
                    () =>
                      openFileHandler
                        ? openFileHandler({
                            source,
                            sourceId: source.id,
                            relativePath: entry.relativePath,
                            name: entry.name,
                          })
                        : previewFile(source, entry.relativePath, entry.name),
                    "file",
                  ],
              isDir
                ? [
                    "Unselect folder contents",
                    () =>
                      mutate("setDirectorySelected", {
                        sourceId: source.id,
                        relativePath: entry.relativePath,
                        selected: false,
                      }),
                    "close",
                  ]
                : [
                    entry.checked
                      ? "Remove from selection"
                      : "Select for context",
                    () =>
                      mutate("setFileSelected", {
                        sourceId: source.id,
                        relativePath: entry.relativePath,
                        selected: !entry.checked,
                      }),
                    entry.checked ? "close" : "check",
                  ],
              isDir
                ? [
                    "Expand",
                    () => {
                      if (children.hidden) caret.click();
                    },
                    "arrow",
                  ]
                : [
                    "Open file",
                    () => previewFile(source, entry.relativePath, entry.name),
                    "file",
                  ],
              isDir
                ? [
                    "Collapse",
                    () => {
                      if (!children.hidden) caret.click();
                    },
                    "arrow",
                  ]
                : null,
              [
                "Refresh source",
                () => mutate("refresh", { sourceId: source.id }),
                "refresh",
              ],
            ].filter(Boolean)),
        },
        checkbox(entry.checked, entry.partial, (selected) =>
          isDir
            ? mutate("setDirectorySelected", {
                sourceId: source.id,
                relativePath: entry.relativePath,
                selected,
              })
            : mutate("setFileSelected", {
                sourceId: source.id,
                relativePath: entry.relativePath,
                selected,
              }),
        ),
        caret,
        icon(isDir ? "folder" : "file", 14),
        extPill,
        name,
        ctxPill,
        isDir ? null : el("small", { class: "explorer-size" }, formatBytes(entry.size)),
      );

      name.addEventListener("click", (event) => {
        if (isDir) {
          caret.click();
          return;
        }
        if (openFileHandler) {
          event.stopPropagation();
          host
            ?.querySelectorAll(".explorer-row.is-selected")
            .forEach((node) => node.classList.remove("is-selected"));
          row.classList.add("is-selected");
          return;
        }
        void previewFile(source, entry.relativePath, entry.name);
      });
      if (!isDir) {
        row.addEventListener("click", () => {
          if (!openFileHandler) return;
          host
            ?.querySelectorAll(".explorer-row.is-selected")
            .forEach((node) => node.classList.remove("is-selected"));
          row.classList.add("is-selected");
        });
        row.addEventListener("dblclick", (event) => {
          event.stopPropagation();
          if (openFileHandler) {
            void openFileHandler({
              sessionId: session?.sessionId,
              source,
              sourceId: source.id,
              relativePath: entry.relativePath,
              name: entry.name,
            });
          } else {
            void previewFile(source, entry.relativePath, entry.name);
          }
        });
      }

      if (isDir) {
        const expanded =
          source.expanded?.includes(entry.relativePath) || false;
        if (expanded) {
          children.hidden = false;
          caret.classList.add("is-open");
          void loadChildren(source, entry.relativePath, children);
        }
        return el("div", { class: "explorer-branch" }, row, children);
      }
      return row;
    };

    const loadChildren = async (source, relativePath, container) => {
      container.dataset.loaded = "1";
      container.replaceChildren(
        el("p", { class: "muted explorer-loading" }, "Loading…"),
      );
      try {
        const result = await call("listDirectory", {
          sourceId: source.id,
          relativePath: relativePath || "",
        });
        if (result?.error) throw new Error(result.error);
        const entries = result.entries || [];
        if (!entries.length) {
          container.replaceChildren(
            el("p", { class: "muted explorer-empty" }, "Empty folder."),
          );
          return;
        }
        const depth = relativePath ? relativePath.split("/").length + 1 : 1;
        container.replaceChildren(
          ...entries.map((entry) => renderRow(source, entry, depth)),
        );
        if (result.truncated) {
          container.append(
            el(
              "p",
              { class: "hint explorer-empty" },
              "This folder has many entries; showing the first 500.",
            ),
          );
        }
      } catch (error) {
        container.replaceChildren(
          el("p", { class: "error-text explorer-empty" }, error.message),
        );
      }
    };

    const sourceSelection = (source) => {
      if (source.kind === "file") return [source.selected, false];
      if (source.selected) {
        return [
          !(source.deselectedPaths || []).length,
          Boolean((source.deselectedPaths || []).length),
        ];
      }
      return [false, Boolean((source.selectedPaths || []).length)];
    };

    const renderSource = (source) => {
      const [checked, partial] = sourceSelection(source);
      const rootChildren = el("div", { class: "explorer-children" });
      const cleanSourcePath = cleanPath(source.path);
      const badge = source.projectKind
        ? el("span", { class: "explorer-badge" }, source.projectKind)
        : el(
            "span",
            { class: "explorer-badge" },
            source.kind === "file" ? "file" : "root",
          );
      const ctxBadge = checked
        ? el(
            "span",
            { class: "explorer-ctx-badge", title: "All files in context" },
            "CTX",
          )
        : partial
          ? el(
              "span",
              { class: "explorer-ctx-badge", title: "Selected files in context" },
              "CTX",
            )
          : null;
      const rootCaret =
        source.kind === "folder"
          ? button(
              "",
              () => {
                const expanded = rootChildren.hidden;
                rootChildren.hidden = !expanded;
                rootCaret.classList.toggle("is-open", expanded);
              },
              "quiet explorer-caret is-open",
              "arrow",
            )
          : null;
      const card = el(
        "div",
        {
          class:
            "explorer-source" +
            (source.missing ? " is-missing" : "") +
            (checked ? " is-in-context" : "") +
            (partial ? " is-partial-context" : ""),
          onContextmenu: (event) =>
            showMenu(event, [
              [
                source.selected ? "Unselect source" : "Select source",
                () =>
                  mutate("setSourceSelected", {
                    sourceId: source.id,
                    selected: !source.selected,
                  }),
                source.selected ? "close" : "check",
              ],
              [
                "Refresh source",
                () => mutate("refresh", { sourceId: source.id }),
                "refresh",
              ],
              [
                "Remove source (files stay on disk)",
                () => mutate("removeSource", { sourceId: source.id }),
                "trash",
              ],
            ]),
        },
        el(
          "div",
          { class: "explorer-source-head" },
          checkbox(checked, partial, (selected) =>
            mutate("setSourceSelected", {
              sourceId: source.id,
              selected,
            }),
          ),
          rootCaret,
          icon(source.kind === "file" ? "file" : "folder", 15),
          el(
            "div",
            { class: "grow explorer-source-meta" },
            el(
              "div",
              { class: "row explorer-source-title" },
              el(
                "span",
                { class: "explorer-source-label", title: cleanSourcePath },
                source.label,
              ),
              badge,
              ctxBadge,
            ),
            el(
              "small",
              { class: "path explorer-source-path", title: cleanSourcePath },
              cleanSourcePath,
            ),
          ),
          ib("Refresh source", "refresh", () =>
            mutate("refresh", { sourceId: source.id }),
          ),
          ib("Remove source", "close", () =>
            mutate("removeSource", { sourceId: source.id }),
          ),
        ),
        source.missing
          ? el(
              "p",
              { class: "error-text explorer-empty" },
              "Folder unavailable. Its registration is kept; restore access or remove it.",
            )
          : source.kind === "folder"
            ? rootChildren
            : el(
                "p",
                { class: "hint explorer-empty" },
                "Individual file source.",
              ),
      );

      if (source.kind === "folder" && !source.missing) {
        card
          .querySelector(".explorer-source-label")
          ?.addEventListener("click", () => rootCaret?.click());
        void loadChildren(source, "", rootChildren);
      } else if (source.kind === "file" && !source.missing) {
        card
          .querySelector(".explorer-source-label")
          ?.addEventListener("click", () =>
            previewFile(source, "", source.label),
          );
      }
      return card;
    };

    const paint = () => {
      if (!host || !data) return;
      const sources = data.sources || [];
      const toolbar = el(
        "div",
        { class: "explorer-toolbar" },
        button("Open Folder", addFolder, "quiet", "folder"),
        compactMode ? null : button("Add folder", addFolder, "quiet", "folder"),
        button(compactMode ? "Select files" : "Add files", addFiles, "quiet", "file"),
        button("Refresh", () => reload(), "quiet", "refresh"),
        compactMode ? null : button("Select all", () => mutate("selectAll", { selected: true }), "quiet", "check"),
        button("Clear", () => mutate("selectAll", { selected: false }), "quiet", "close"),
      );
      const preferences = el(
        "div",
        { class: "explorer-preferences" },
        el(
          "label",
          { class: "explorer-toggle" },
          el("input", {
            type: "checkbox",
            checked: data.preferences?.showHidden,
            onChange: (event) =>
              mutate("setPreferences", {
                showHidden: event.target.checked,
              }),
          }),
          " Hidden",
        ),
        el(
          "label",
          { class: "explorer-toggle" },
          el("input", {
            type: "checkbox",
            checked: data.preferences?.showIgnored,
            onChange: (event) =>
              mutate("setPreferences", {
                showIgnored: event.target.checked,
              }),
          }),
          " Ignored / excluded",
        ),
        button("Context preview", openPreview, "quiet", "spark"),
        el("input", {
          class: "explorer-excludes",
          type: "text",
          placeholder: "Extra excludes: *.log, tmp/",
          title: "Comma-separated names or relative paths that folder selection should skip.",
          value: (data.preferences?.extraExcludes || []).join(", "),
          onChange: (event) =>
            mutate("setPreferences", {
              extraExcludes: event.target.value
                .split(",")
                .map((value) => value.trim())
                .filter(Boolean),
            }),
        }),
      );
      const selectedCount = data.selectedFiles || 0;
      const estTokens =
        preview?.estimatedTokens ??
        Math.ceil((preview?.estimatedChars || 0) / 4);
      const budgetTokens =
        preview?.budgetTokens ??
        Math.ceil((preview?.budgetChars || 200000) / 4);
      const includedFiles =
        preview?.includedFiles ?? (preview?.selectedFiles ?? selectedCount);
      const omittedFiles = preview?.omittedFiles ?? 0;
      const summary = el(
        "div",
        { class: "explorer-summary" },
        el(
          "span",
          { class: "explorer-count" },
          `${selectedCount} file${selectedCount === 1 ? "" : "s"} selected · ${includedFiles} in prompt · ~${estTokens} / ${budgetTokens} tokens`,
        ),
        el("small", { class: "hint" }, `Context: ${projectName} · ${session.title || "Untitled"}`),
      );
      const warnings = [];
      if (data.selectionCapped) {
        warnings.push(
          "Selection is large; counts are capped. Narrow the selection for a precise context.",
        );
      }
      if (preview?.exceedsBudget) {
        warnings.push(
          `Estimated context exceeds the budget (${preview.estimatedChars} of ${preview.budgetChars} chars · ~${estTokens} / ${budgetTokens} tokens; ${includedFiles} included, ${omittedFiles} omitted). Files will be truncated. Uncheck the root folder to select individual files.`,
        );
      }
      if (preview?.missingSources?.length) {
        warnings.push(
          `${preview.missingSources.length} source(s) are unavailable and were skipped.`,
        );
      }
      const body = el(
        "div",
        { class: "explorer-sources" },
        ...(sources.length
          ? sources.map(renderSource)
          : [
              el(
                "div",
                { class: "explorer-empty-state" },
                icon("folder", 24),
                el("strong", {}, "No sources yet"),
                el(
                  "p",
                  { class: "hint" },
                  "Add a folder or individual files. Each source stays independent and is only shared when you select it for this conversation.",
                ),
                workspaceFolders.length
                  ? button(
                      "Open workspace folder",
                      async () => {
                        for (const folderPath of workspaceFolders) {
                          if (!folderPath) continue;
                          await mutate("addSource", { path: folderPath });
                        }
                      },
                      "quiet",
                      "folder",
                    )
                  : null,
              ),
            ]),
      );
      host.replaceChildren(
        el(
          "div",
          { class: "explorer-panel" },
          el(
            "div",
            { class: "explorer-head" },
            el("strong", { class: "explorer-title" }, "EXPLORER"),
            el("small", { class: "hint" }, "Per-conversation context"),
          ),
          summary,
          toolbar,
          preferences,
          ...warnings.map((text) =>
            el("p", { class: "explorer-warning" }, text),
          ),
          body,
        ),
      );
    };

    const addFolder = async () => {
      try {
        const path = await rpc("FileSystem", "browseFolder", [
          "Add a folder to the Explorer",
        ]);
        if (!path) return;
        await applyState(await call("addSource", { path }));
      } catch (error) {
        toast(error.message, true);
      }
    };

    const addFiles = async () => {
      try {
        const paths = await rpc("FileSystem", "browseFiles", [
          "Add files to the Explorer",
        ]);
        if (!paths?.length) return;
        await applyState(await call("addSources", { paths }));
        if (data?.warnings?.length) toast(data.warnings[0], true);
      } catch (error) {
        toast(error.message, true);
      }
    };

    const openPreview = async () => {
      try {
        await refreshPreview();
        const info = preview || {};
        const estTokens =
          info.estimatedTokens ?? Math.ceil((info.estimatedChars || 0) / 4);
        const budgetTokens =
          info.budgetTokens ?? Math.ceil((info.budgetChars || 200000) / 4);
        const includedFiles =
          info.includedFiles ?? (info.selectedFiles || 0);
        const omittedFiles = info.omittedFiles ?? 0;
        const lines = [
          `${info.selectedFiles || 0} file(s) selected (${includedFiles} included in prompt${omittedFiles ? `, ${omittedFiles} omitted` : ""}) · ${formatBytes(info.estimatedBytes || 0)} · ~${info.estimatedChars || 0} chars (~${estTokens} tokens)`,
          `Budget: ${info.budgetChars || 0} chars (~${budgetTokens} tokens)`,
          info.capped
            ? "The selection walk hit its safety cap; some files may be omitted."
            : null,
          info.exceedsBudget
            ? "This selection exceeds the budget and will be truncated when sent."
            : "This selection fits the current budget.",
          ...(info.missingSources || []).map(
            (path) => `Unavailable source: ${path}`,
          ),
        ].filter(Boolean);
        dialog(
          "Explorer context",
          el(
            "div",
            {},
            ...lines.map((line, index) =>
              el(
                "p",
                {
                  class:
                    index > 0 && info.exceedsBudget
                      ? "error-text"
                      : "hint",
                },
                line,
              ),
            ),
            el(
              "p",
              { class: "hint" },
              "Only these files are sent to the agent. Other conversations and projects are never included.",
            ),
          ),
        );
      } catch (error) {
        toast(error.message, true);
      }
    };

    const reload = async (options = {}) => {
      if (!host?.isConnected) return;
      try {
        data = await call("getState");
        await refreshPreview();
        paint();
      } catch (error) {
        if (!options.quiet) toast(error.message, true);
      }
    };

    return {
      async render(target, currentSession, meta = {}) {
        host = target;
        session = currentSession;
        projectName = meta.projectName || "No project";
        compactMode = Boolean(meta.compact);
        if (typeof meta.onOpenFile === "function") {
          openFileHandler = meta.onOpenFile;
        }
        workspaceFolders = Array.isArray(meta.workspaceFolders)
          ? meta.workspaceFolders.filter(Boolean)
          : [];
        host.replaceChildren(
          el("p", { class: "muted" }, "Loading Explorer…"),
        );
        await reload();
        clearInterval(timer);
        timer = setInterval(() => {
          if (!host?.isConnected) {
            clearInterval(timer);
            timer = null;
            return;
          }
          if (!document.hidden) void reload({ quiet: true });
        }, 15000);
      },
      dispose() {
        clearInterval(timer);
        timer = null;
        closeMenu();
      },
    };
  }

  window.TsExplorerPanel = { create };
})();
