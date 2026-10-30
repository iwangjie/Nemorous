// Nemorous code line statistics plugin.

var MAX_FILES = 2500;
var MAX_BYTES = 2 << 20;
var SKIP_DIRS = {
  ".git": 1, "node_modules": 1, "vendor": 1, "dist": 1, "build": 1, "target": 1,
  ".idea": 1, ".next": 1, "out": 1, "coverage": 1, "venv": 1, ".venv": 1,
  "__pycache__": 1, "Pods": 1, ".gradle": 1, ".mvn": 1, "bin": 1, "tmp": 1
};
var LANG = {
  go: "Go", js: "JavaScript", mjs: "JavaScript", cjs: "JavaScript", jsx: "JavaScript",
  ts: "TypeScript", tsx: "TypeScript", java: "Java", kt: "Kotlin", kts: "Kotlin",
  py: "Python", rs: "Rust", c: "C", h: "C/C++", cc: "C++", cpp: "C++", hpp: "C++",
  cs: "C#", rb: "Ruby", php: "PHP", swift: "Swift", m: "Objective-C", mm: "Objective-C",
  sh: "Shell", bash: "Shell", zsh: "Shell", fish: "Shell", sql: "SQL",
  html: "HTML", css: "CSS", scss: "SCSS", sass: "SCSS", less: "Less",
  vue: "Vue", svelte: "Svelte", dart: "Dart", lua: "Lua", r: "R", scala: "Scala",
  yaml: "YAML", yml: "YAML", json: "JSON", toml: "TOML", xml: "XML"
};

var state = {
  loaded: false,
  scanning: false,
  total: 0,
  code: 0,
  comments: 0,
  blanks: 0,
  files: 0,
  languages: [],
  dirs: [],
  top: [],
  error: "",
  took: ""
};

function onWorkspaceChanged() {
  state.loaded = false;
  state.languages = [];
  state.dirs = [];
  state.top = [];
  state.error = "";
  nemorous.invalidate();
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

function normalize(path) {
  return String(path || "").replace(/\\/g, "/");
}

function relative(root, path) {
  var r = normalize(root);
  var p = normalize(path);
  if (r && p.indexOf(r + "/") === 0) return p.slice(r.length + 1);
  return p;
}

function extension(name) {
  var i = name.lastIndexOf(".");
  return i >= 0 ? name.slice(i + 1).toLowerCase() : "";
}

function isComment(line) {
  var t = line.trim();
  if (!t) return false;
  return t.indexOf("//") === 0 || t.indexOf("#") === 0 || t.indexOf("--") === 0 ||
    t.indexOf("/*") === 0 || t.indexOf("*") === 0 || t.indexOf("<!--") === 0 ||
    t.indexOf(";") === 0 || t.indexOf("%") === 0 || t.indexOf("REM ") === 0;
}

function walk(dir, depth, out) {
  if (out.length >= MAX_FILES || depth > 9) return;
  var entries;
  try {
    entries = nemorous.fs.list(dir);
  } catch (e) {
    return;
  }
  for (var i = 0; i < entries.length && out.length < MAX_FILES; i++) {
    var entry = entries[i];
    if (entry.dir) {
      if (entry.name.charAt(0) === "." || SKIP_DIRS[entry.name] === 1) continue;
      walk(entry.path, depth + 1, out);
    } else if (LANG[extension(entry.name)] && entry.size <= MAX_BYTES) {
      out.push(entry);
    }
  }
}

function resetStat() {
  state.total = 0;
  state.code = 0;
  state.comments = 0;
  state.blanks = 0;
  state.files = 0;
  state.languages = [];
  state.dirs = [];
  state.top = [];
}

function scan() {
  var workspace = nemorous.workspace();
  var start = Date.now();
  state.scanning = true;
  state.error = "";
  resetStat();
  if (!workspace.path) {
    state.scanning = false;
    state.loaded = true;
    state.error = "请先打开工作区";
    return;
  }
  var entries = [];
  walk(workspace.path, 0, entries);
  var byLang = {};
  var byDir = {};
  var files = [];
  for (var i = 0; i < entries.length; i++) {
    var entry = entries[i];
    var content;
    try {
      content = nemorous.fs.read(entry.path);
    } catch (e) {
      continue;
    }
    if (content.indexOf("\u0000") >= 0) continue;
    var lines = content.split(/\r?\n/);
    var code = 0, comments = 0, blanks = 0;
    for (var j = 0; j < lines.length; j++) {
      if (!lines[j].trim()) {
        blanks++;
      } else if (isComment(lines[j])) {
        comments++;
      } else {
        code++;
      }
    }
    var total = lines.length;
    if (lines.length > 0 && lines[lines.length - 1] === "") total--;
    state.files++;
    state.total += total;
    state.code += code;
    state.comments += comments;
    state.blanks += blanks;
    var lang = LANG[extension(entry.name)];
    if (!byLang[lang]) byLang[lang] = { name: lang, lines: 0, files: 0 };
    byLang[lang].lines += total;
    byLang[lang].files++;
    var relPath = relative(workspace.path, entry.path);
    var dirName = relPath.indexOf("/") >= 0 ? relPath.split("/")[0] : "(根目录)";
    if (!byDir[dirName]) byDir[dirName] = { name: dirName, lines: 0, files: 0 };
    byDir[dirName].lines += total;
    byDir[dirName].files++;
    files.push({ path: relPath, lines: total, lang: lang });
  }
  var languages = [];
  for (var name in byLang) {
    if (!byLang.hasOwnProperty(name)) continue;
    languages.push(byLang[name]);
  }
  languages.sort(function (a, b) { return b.lines - a.lines; });
  var dirs = [];
  for (var dn in byDir) {
    if (!byDir.hasOwnProperty(dn)) continue;
    dirs.push(byDir[dn]);
  }
  dirs.sort(function (a, b) { return b.lines - a.lines; });
  files.sort(function (a, b) { return b.lines - a.lines; });
  state.languages = languages;
  state.dirs = dirs;
  state.top = files.slice(0, 16);
  state.scanning = false;
  state.loaded = true;
  state.took = ((Date.now() - start) / 1000).toFixed(2) + "s";
}

function percent(n) {
  return state.total > 0 ? Math.round(n * 100 / state.total) : 0;
}

function handleAction(action, payload) {
  if (action === "open" || action === "refresh") scan();
  nemorous.invalidate();
}

function summary() {
  return el("row", { gap: 5, children: [
    el("badge", { text: state.total.toLocaleString() + " 行", tone: "accent" }),
    el("badge", { text: state.code.toLocaleString() + " 代码", tone: "ok" }),
    el("badge", { text: state.comments.toLocaleString() + " 注释", tone: "warn" }),
    el("badge", { text: state.files + " 文件", tone: "muted" })
  ] });
}

function render(panelId) {
  if (!state.loaded && !state.scanning) scan();
  var root = el("column", { grow: true, gap: 7, padding: 8, children: [] });
  root.children.push(el("row", { gap: 5, children: [
    btn("重新扫描", "refresh", null, true, state.scanning),
    el("spacer"),
    txt(state.took ? "耗时 " + state.took : "", { size: 9.5, muted: true })
  ] }));
  if (state.error) root.children.push(txt(state.error, { tone: "err", size: 10 }));
  if (state.total === 0) {
    root.children.push(txt("没有可统计的代码文件", { muted: true, size: 11 }));
    return root;
  }
  root.children.push(summary());
  root.children.push(txt("语言分布", { size: 10.5, bold: true }));
  for (var i = 0; i < state.languages.length && i < 8; i++) {
    var l = state.languages[i];
    root.children.push(el("bar", {
      text: l.name,
      detail: l.lines.toLocaleString() + " 行 · " + percent(l.lines) + "%",
      fraction: l.lines / state.total,
      tone: i === 0 ? "accent" : ""
    }));
  }
  if (state.dirs.length > 1) {
    root.children.push(txt("目录分布", { size: 10.5, bold: true }));
    for (var d = 0; d < state.dirs.length && d < 6; d++) {
      var dir = state.dirs[d];
      root.children.push(el("bar", {
        text: dir.name,
        detail: dir.lines.toLocaleString() + " 行 · " + dir.files + " 文件",
        fraction: dir.lines / state.total,
        tone: d === 0 ? "accent" : ""
      }));
    }
  }
  root.children.push(el("divider"));
  root.children.push(txt("最大文件", { size: 10.5, bold: true }));
  var items = [];
  for (var j = 0; j < state.top.length; j++) {
    var f = state.top[j];
    items.push({ text: f.path, detail: f.lines.toLocaleString() + " 行", mono: true });
  }
  root.children.push(el("list", { items: items, scroll: true, grow: true }));
  return root;
}
