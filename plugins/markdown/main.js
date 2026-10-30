// Nemorous Markdown editor integration.
//
// Opening a Markdown file in the code editor switches that tab to this view:
// source and preview side by side, preview only, or live preview while typing.
// There is no separate side tool window; the file stays the editor's subject.

var state = {
  path: "",
  source: "",
  saved: "",
  drafts: {},
  mode: "split",
  live: true,
  dirty: false,
  status: "",
  error: "",
  rev: 0
};

function activate() {
  var prefs = nemorous.storage.get("prefs");
  if (prefs && typeof prefs === "object") {
    if (prefs.mode === "split" || prefs.mode === "preview") state.mode = prefs.mode;
    if (typeof prefs.live === "boolean") state.live = prefs.live;
  }
}

function onWorkspaceChanged() {
  state.path = "";
  state.source = "";
  state.saved = "";
  state.drafts = {};
  state.dirty = false;
  state.error = "";
  state.status = "";
  nemorous.invalidate();
}

function savePrefs() {
  nemorous.storage.set("prefs", { mode: state.mode, live: state.live });
}

function el(type, props) {
  var n = props || {};
  n.type = type;
  return n;
}

function txt(value, props) {
  var n = props || {};
  n.type = "text";
  n.text = String(value == null ? "" : value);
  return n;
}

function btn(label, action, payload, primary, disabled, small) {
  return el("button", { text: label, action: action, payload: payload, primary: !!primary, disabled: !!disabled, small: small !== false });
}

function isMarkdown(path) {
  return /\.(md|markdown|mdown|mkd)$/i.test(path || "");
}

function baseName(path) {
  var p = String(path || "").replace(/\\/g, "/");
  var i = p.lastIndexOf("/");
  return i >= 0 ? p.slice(i + 1) : p;
}

function dirName(path) {
  var p = String(path || "").replace(/\\/g, "/");
  var i = p.lastIndexOf("/");
  return i > 0 ? p.slice(0, i) : p;
}

function activeMarkdownFile() {
  var file = null;
  try {
    file = nemorous.editor.activeFile();
  } catch (e) {
    return null;
  }
  if (file && isMarkdown(file.path || file.relPath)) return file;
  return null;
}

function load(file) {
  if (!file) {
    state.path = "";
    state.source = "";
    state.saved = "";
    state.dirty = false;
    state.error = "";
    state.status = "";
    return;
  }
  if (state.path && state.path !== file.path && state.dirty) {
    state.drafts[state.path] = state.source;
  }
  var disk = "";
  try {
    disk = nemorous.fs.read(file.path);
  } catch (e) {
    disk = typeof file.content === "string" ? file.content : "";
  }
  var draft = state.drafts[file.path];
  state.rev++;
  state.path = file.path;
  state.saved = disk;
  state.source = typeof draft === "string" ? draft : (typeof file.content === "string" ? file.content : disk);
  state.dirty = state.source !== state.saved;
  state.status = baseName(file.path);
  state.error = "";
}

function openActive() {
  var file = activeMarkdownFile();
  if (file) {
    if (state.path !== file.path || !state.dirty) load(file);
  } else {
    load(null);
  }
}

function reload() {
  var file = activeMarkdownFile();
  if (file) load(file);
}

function save() {
  if (!state.path) return;
  try {
    nemorous.fs.write(state.path, state.source);
    state.saved = state.source;
    state.dirty = false;
    delete state.drafts[state.path];
    state.status = "已保存 · " + baseName(state.path);
    state.error = "";
    nemorous.notify("ok", "Markdown 已保存");
  } catch (e) {
    state.error = "保存失败：" + String(e);
  }
}

function previewText() {
  return state.live ? state.source : state.saved;
}

function documentStats() {
  var text = state.source || "";
  var lines = text === "" ? 0 : text.split(/\r?\n/).length;
  var chars = text.replace(/\s/g, "").length;
  return lines + " 行 · " + chars + " 字符";
}

function toolbar() {
  return el("row", {
    gap: 4,
    children: [
      btn("双栏", "mode", { mode: "split" }, state.mode === "split"),
      btn("预览", "mode", { mode: "preview" }, state.mode === "preview"),
      el("checkbox", { key: "live", text: "实时", checked: state.live, action: "live" }),
      el("spacer"),
      btn("保存", "save", null, true, !state.dirty),
      btn("重载", "reload", null, false)
    ]
  });
}

function handleAction(action, payload) {
  var p = payload || {};
  switch (action) {
    case "open":
    case "openActive":
      openActive();
      break;
    case "activeFileChanged":
      openActive();
      break;
    case "reload":
      reload();
      break;
    case "input":
      if (p.key && String(p.key).indexOf("src:") === 0) {
        state.source = String(p.value == null ? "" : p.value);
        state.dirty = state.source !== state.saved;
        if (state.path && state.dirty) state.drafts[state.path] = state.source;
        else if (state.path) delete state.drafts[state.path];
      }
      break;
    case "mode":
      if (p.mode === "split" || p.mode === "preview") {
        state.mode = p.mode;
        savePrefs();
      }
      break;
    case "live":
      state.live = !!p.checked;
      savePrefs();
      break;
    case "save":
      save();
      break;
  }
  nemorous.invalidate();
}

function render(panelId) {
  var root = el("column", { grow: true, gap: 6, padding: 8, children: [] });
  if (!state.path) {
    root.children.push(el("column", {
      grow: true, gap: 6, children: [
        txt("打开 Markdown 文件开始预览", { size: 12, bold: true }),
        txt("与普通编辑器一样打开 .md，即可自动进入此视图", { size: 10, muted: true })
      ]
    }));
    return root;
  }
  root.children.push(toolbar());
  if (state.status) root.children.push(txt(state.status + " · " + documentStats() + (state.dirty ? " · 未保存" : ""), { size: 10, muted: true, mono: true }));
  if (state.error) root.children.push(txt(state.error, { tone: "err", size: 10 }));
  if (state.mode === "split") {
    root.children.push(el("row", {
      grow: true,
      gap: 6,
      children: [
        el("codeeditor", {
          key: "src:" + state.rev + ":" + state.path,
          value: state.source,
          language: "markdown",
          softWrap: true,
          action: "input",
          widthPercent: 50,
          heightPercent: 100
        }),
        el("divider", { vertical: true }),
        el("markdown", {
          text: previewText(),
          basePath: dirName(state.path),
          widthPercent: 50,
          heightPercent: 100
        })
      ]
    }));
  } else {
    root.children.push(el("markdown", { text: previewText(), basePath: dirName(state.path), grow: true }));
  }
  return root;
}
