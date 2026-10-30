// Nemorous CSV / TSV editor integration.
//
// Opening a .csv or .tsv file in the code editor switches that tab to a
// native table. The file stays the editor tab's subject; the table is a
// read-only view of its contents.

var MAX_ROWS = 2000;
var MAX_COLS = 80;

var state = {
  path: "",
  headers: [],
  rows: [],
  query: "",
  delimiter: ",",
  rev: 0,
  error: ""
};

function onWorkspaceChanged() {
  state.path = "";
  state.headers = [];
  state.rows = [];
  state.query = "";
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

function isCSV(path) {
  return /\.(csv|tsv|tab)$/i.test(path || "");
}

function baseName(path) {
  var p = String(path || "").replace(/\\/g, "/");
  var i = p.lastIndexOf("/");
  return i >= 0 ? p.slice(i + 1) : p;
}

function activeCSVFile() {
  var file = null;
  try {
    file = nemorous.editor.activeFile();
  } catch (e) {
    return null;
  }
  if (file && isCSV(file.path || file.relPath)) return file;
  return null;
}

function detectDelimiter(text, path) {
  if (/\.(tsv|tab)$/i.test(path || "")) return "\t";
  var line = text.split(/\r?\n/)[0] || "";
  var commas = (line.match(/,/g) || []).length;
  var tabs = (line.match(/\t/g) || []).length;
  var semis = (line.match(/;/g) || []).length;
  if (tabs > commas && tabs >= semis) return "\t";
  if (semis > commas) return ";";
  return ",";
}

function parse(text, delimiter) {
  var rows = [];
  var row = [];
  var field = "";
  var quoted = false;
  for (var i = 0; i < text.length; i++) {
    var ch = text.charAt(i);
    if (quoted) {
      if (ch === '"') {
        if (text.charAt(i + 1) === '"') {
          field += '"';
          i++;
        } else {
          quoted = false;
        }
      } else {
        field += ch;
      }
    } else if (ch === '"') {
      quoted = true;
    } else if (ch === delimiter) {
      row.push(field);
      field = "";
    } else if (ch === "\n") {
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
      if (rows.length >= MAX_ROWS + 1) break;
    } else if (ch !== "\r") {
      field += ch;
    }
  }
  if (field.length > 0 || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

function load(file) {
  if (!file) {
    state.path = "";
    state.headers = [];
    state.rows = [];
    state.query = "";
    state.error = "";
    return;
  }
  var text = "";
  try {
    text = nemorous.fs.read(file.path);
  } catch (e) {
    if (typeof file.content === "string") text = file.content;
    else {
      state.error = "读取失败：" + String(e);
      return;
    }
  }
  var delimiter = detectDelimiter(text, file.path);
  var parsed = parse(text, delimiter);
  if (parsed.length === 0) {
    state.error = "文件为空";
    return;
  }
  var headerRow = parsed[0];
  var headers = [];
  for (var c = 0; c < headerRow.length && c < MAX_COLS; c++) {
    headers.push(headerRow[c] || ("列 " + (c + 1)));
  }
  var rows = [];
  for (var r = 1; r < parsed.length && rows.length < MAX_ROWS; r++) {
    var cells = [];
    for (var j = 0; j < headers.length; j++) {
      var value = parsed[r][j] || "";
      if (value.length > 400) value = value.slice(0, 400) + "…";
      cells.push(value);
    }
    rows.push(cells);
  }
  state.rev++;
  state.path = file.path;
  state.headers = headers;
  state.rows = rows;
  state.query = "";
  state.delimiter = delimiter;
  state.error = "";
}

function openActive() {
  var file = activeCSVFile();
  if (!file) {
    load(null);
    return;
  }
  if (state.path !== file.path) load(file);
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
    case "refresh":
      var file = activeCSVFile();
      if (file) load(file);
      break;
    case "search":
      state.query = String(p.value == null ? "" : p.value);
      break;
  }
  nemorous.invalidate();
}

function visibleRows() {
  var query = state.query.toLowerCase();
  if (!query) return state.rows;
  var out = [];
  for (var i = 0; i < state.rows.length; i++) {
    var line = state.rows[i].join(" ").toLowerCase();
    if (line.indexOf(query) >= 0) out.push(state.rows[i]);
  }
  return out;
}

function render(panelId) {
  var root = el("column", { grow: true, gap: 6, padding: 8, children: [] });
  if (!state.path) {
    root.children.push(el("column", {
      grow: true, gap: 6, children: [
        txt("打开 CSV / TSV 文件开始预览", { size: 12, bold: true }),
        txt("与普通编辑器一样打开表格文件，即可自动进入此视图", { size: 10, muted: true })
      ]
    }));
    return root;
  }
  root.children.push(el("row", { gap: 4, children: [
    el("input", { key: "csv-search:" + state.rev, value: state.query, placeholder: "搜索单元格", action: "search", grow: true }),
    btn("刷新", "refresh", null, false)
  ] }));
  if (state.error) root.children.push(txt(state.error, { tone: "err", size: 10 }));
  var rows = visibleRows();
  var delimiter = state.delimiter === "\t" ? "TSV" : "CSV";
  root.children.push(el("row", { gap: 6, children: [
    txt(baseName(state.path), { size: 11, bold: true }),
    txt(delimiter + " · " + state.rows.length + " 行 × " + state.headers.length + " 列", { size: 9.5, muted: true }),
    el("spacer"),
    txt(state.query ? rows.length + " 个匹配" : "", { size: 9.5, muted: true })
  ] }));
  if (rows.length === 0) {
    root.children.push(txt("没有匹配的行", { muted: true, size: 10.5 }));
    return root;
  }
  root.children.push(el("table", { headers: state.headers, rows: rows, grow: true }));
  return root;
}
