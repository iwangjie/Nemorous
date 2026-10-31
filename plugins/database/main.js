// Database workbench: MySQL / PostgreSQL / Oracle / SQLite over the host's
// database/sql bridge. Credentials stay in plugin storage.

var MAX_HISTORY = 24;
var state = {
  connections: [],
  history: [],
  selected: "",
  view: "query",
  sql: "SELECT 1",
  sqlKey: 0,
  result: null,
  execResult: null,
  running: false,
  status: "",
  statusTone: "",
  error: "",
  tables: [],
  tablesFor: "",
  selectedTable: "",
  columns: [],
  indexes: [],
  schemaLoading: false,
  schemaSeq: 0,
  editing: null,
  form: emptyForm(),
  formKey: 0
};

function emptyForm() {
  return { id: "", name: "", driver: "sqlite", host: "", port: "", database: "", user: "", password: "", path: "", schema: "", sslmode: "disable", dsn: "" };
}
function activate() {
  var conns = nemorous.storage.get("connections");
  if (conns && conns.length) state.connections = conns;
  var history = nemorous.storage.get("history");
  if (history && history.length) state.history = history;
  var selected = nemorous.storage.get("selected");
  if (selected) state.selected = selected;
  var sql = nemorous.storage.get("sql");
  if (typeof sql === "string" && sql) state.sql = sql;
  if (!state.connections.length) state.view = "connections";
  else if (!findConnection(state.selected)) state.selected = state.connections[0].id;
}
function saveConnections() { nemorous.storage.set("connections", state.connections); }
function saveHistory() { nemorous.storage.set("history", state.history); }

// ---------- helpers ----------
function el(type, props) { var n = props || {}; n.type = type; return n; }
function text(value, props) { var n = props || {}; n.type = "text"; n.text = String(value == null ? "" : value); return n; }
function hint(value) { return text(value, { muted: true, size: 10.5 }); }
function badge(value, tone) { return el("badge", { text: String(value), tone: tone || "" }); }
function btn(label, action, payload, props) {
  var n = props || {};
  n.type = "button"; n.text = label; n.action = action;
  if (payload !== null && payload !== undefined) n.payload = payload;
  return n;
}
function input(key, value, placeholder, props) {
  var n = props || {};
  n.type = "input"; n.key = key; n.value = value == null ? "" : String(value);
  n.placeholder = placeholder || ""; n.action = n.action || "input";
  return n;
}
function code(value, props) { var n = props || {}; n.type = "code"; n.text = String(value || ""); n.mono = true; return n; }
function row(children, props) { var n = props || {}; n.type = "row"; n.children = clean(children); return n; }
function col(children, props) { var n = props || {}; n.type = "column"; n.children = clean(children); return n; }
function card(children, props) { var n = props || {}; n.type = "card"; n.children = clean(children); return n; }
function list(items, props) { var n = props || {}; n.type = "list"; n.items = items; return n; }
function clean(children) {
  var out = [];
  for (var i = 0; i < (children || []).length; i++) if (children[i]) out.push(children[i]);
  return out;
}
function spacer() { return { type: "spacer" }; }
function divider() { return { type: "divider" }; }
function sectionTitle(value, extra) { return row([text(value, { bold: true, size: 10.5 }), extra || null, spacer()], { gap: 4 }); }
function tail(s, max) { s = String(s || ""); return s.length > max ? s.substring(s.length - max) : s; }
function baseName(p) {
  var v = String(p || "").replace(/[\\/]+$/, "");
  var i = Math.max(v.lastIndexOf("/"), v.lastIndexOf("\\"));
  return i >= 0 ? v.substring(i + 1) : v;
}
function driverLabel(d) {
  return d === "mysql" ? "MySQL" : d === "postgres" ? "PostgreSQL" : d === "oracle" ? "Oracle" : "SQLite";
}
function defaultPort(d) { return d === "mysql" ? "3306" : d === "postgres" ? "5432" : d === "oracle" ? "1521" : ""; }
function findConnection(id) {
  for (var i = 0; i < state.connections.length; i++) if (state.connections[i].id === id) return state.connections[i];
  return null;
}
function selectedConnection() { return findConnection(state.selected); }
function encode(v) { return encodeURIComponent(String(v == null ? "" : v)); }
function sqlLiteral(v) { return "'" + String(v == null ? "" : v).replace(/'/g, "''") + "'"; }
function quoteIdent(name) {
  var c = selectedConnection();
  var v = String(name || "").replace(/"/g, '""');
  return c && c.driver === "mysql" ? "`" + String(name).replace(/`/g, "``") + "`" : '"' + v + '"';
}
function sqlitePath(c) {
  var p = String(c.path || "").trim();
  if (!p) return "";
  if (/^[A-Za-z]:[\\/]/.test(p) || p.charAt(0) === '/' || p.charAt(0) === "~") return p;
  var ws = nemorous.workspace();
  return (ws.path ? ws.path.replace(/[\\/]+$/, "") + "/" : "") + p;
}
function dsnFor(c) {
  if (!c) return "";
  if (c.dsn && String(c.dsn).trim()) return String(c.dsn).trim();
  if (c.driver === "sqlite") return sqlitePath(c);
  var port = String(c.port || defaultPort(c.driver));
  if (c.driver === "mysql") {
    return String(c.user || "") + ":" + String(c.password || "") + "@tcp(" + c.host + ":" + port + ")/" + encode(c.database) + "?parseTime=true&charset=utf8mb4";
  }
  if (c.driver === "postgres") {
    return "postgres://" + encode(c.user) + ":" + encode(c.password) + "@" + c.host + ":" + port + "/" + encode(c.database || "postgres") + "?sslmode=" + encode(c.sslmode || "disable");
  }
  if (c.driver === "oracle") {
    return "oracle://" + encode(c.user) + ":" + encode(c.password) + "@" + c.host + ":" + port + "/" + encode(c.database || c.path || "ORCL");
  }
  return "";
}
function testSQL(driver) { return driver === "oracle" ? "SELECT 1 FROM DUAL" : "SELECT 1"; }
function isReadSQL(sql) { return /^\s*(select|show|describe|desc|explain|pragma|with|values|table)\b/i.test(sql); }
function errText(e) { return String(e == null ? "unknown error" : e); }

function callDBFor(c, mode, sql, onOK, onErr) {
  if (!c) { onErr("请先选择数据库连接"); return; }
  if (!sql || !String(sql).trim()) { onErr("SQL 不能为空"); return; }
  var dsn = dsnFor(c);
  if (!dsn) { onErr("连接配置不完整"); return; }
  var opts = {
    driver: c.driver,
    dsn: dsn,
    sql: String(sql),
    mode: mode,
    timeoutMs: 30000
  };
  if (mode === "exec") nemorous.db.exec(opts, onOK, onErr);
  else nemorous.db.query(opts, onOK, onErr);
}
function callDB(mode, sql, onOK, onErr) {
  callDBFor(selectedConnection(), mode, sql, onOK, onErr);
}
function setError(message) {
  state.error = String(message || "");
  state.status = "操作失败";
  state.statusTone = "err";
  state.running = false;
  state.schemaLoading = false;
  nemorous.invalidate();
}

// ---------- query ----------
function runQuery() {
  var sql = String(state.sql || "").trim();
  if (!sql) { setError("SQL 不能为空"); return; }
  state.running = true;
  state.error = "";
  state.status = "正在执行查询…";
  state.statusTone = "accent";
  state.result = null;
  state.execResult = null;
  nemorous.storage.set("sql", state.sql);
  var mode = isReadSQL(sql) ? "query" : "exec";
  var started = Date.now();
  callDB(mode, sql, function (result) {
    state.running = false;
    state.status = mode === "query" ? "查询完成" : "执行完成";
    state.statusTone = "ok";
    if (mode === "query") state.result = result;
    else state.execResult = result;
    addHistory(sql, true, Date.now() - started);
    nemorous.invalidate();
  }, function (e) {
    addHistory(sql, false, Date.now() - started);
    setError(errText(e));
  });
}
function addHistory(sql, ok, ms) {
  var c = selectedConnection();
  state.history.unshift({
    sql: sql,
    connection: c ? c.name : "",
    driver: c ? c.driver : "",
    at: new Date().toLocaleString(),
    ok: !!ok,
    ms: ms
  });
  var seen = {};
  var out = [];
  for (var i = 0; i < state.history.length && out.length < MAX_HISTORY; i++) {
    var h = state.history[i];
    var key = h.sql;
    if (seen[key]) continue;
    seen[key] = true;
    out.push(h);
  }
  state.history = out;
  saveHistory();
}
function formatCell(v) {
  if (v === null || v === undefined) return "NULL";
  if (typeof v === "object") return JSON.stringify(v);
  var s = String(v);
  return s.length > 300 ? s.slice(0, 300) + "…" : s;
}
function resultTable() {
  var r = state.result;
  if (!r) return [];
  var headers = (r.columns || []).slice(0, 64);
  var rows = [];
  for (var i = 0; i < r.rows.length && i < 300; i++) {
    var out = [];
    for (var j = 0; j < headers.length; j++) out.push(formatCell(r.rows[i][j]));
    rows.push(out);
  }
  return [el("table", { headers: headers, rows: rows, maxHeight: 420, scroll: true })];
}

// ---------- schema ----------
function tablesQuery(c) {
  if (c.driver === "sqlite") return "SELECT name, type FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name";
  if (c.driver === "mysql") return "SELECT table_name AS name, table_type AS type FROM information_schema.tables WHERE table_schema = DATABASE() ORDER BY table_name";
  if (c.driver === "postgres") return "SELECT table_name AS name, table_type AS type FROM information_schema.tables WHERE table_schema = " + sqlLiteral(c.schema || "public") + " ORDER BY table_name";
  return "SELECT table_name AS name, 'TABLE' AS type FROM all_tables WHERE owner = SYS_CONTEXT('USERENV','CURRENT_SCHEMA') ORDER BY table_name";
}
function columnsQuery(c, table) {
  var t = sqlLiteral(table);
  if (c.driver === "sqlite") return "SELECT name, type, CASE WHEN \"notnull\"=0 THEN 'YES' ELSE 'NO' END AS nullable, dflt_value AS default_value, CASE WHEN pk>0 THEN 'PK' ELSE '' END AS key, '' AS extra FROM pragma_table_info(" + t + ")";
  if (c.driver === "mysql") return "SELECT column_name AS name, column_type AS type, is_nullable AS nullable, column_default AS default_value, column_key AS key, extra FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = " + t + " ORDER BY ordinal_position";
  if (c.driver === "postgres") return "SELECT column_name AS name, data_type AS type, is_nullable AS nullable, column_default AS default_value, '' AS key, '' AS extra FROM information_schema.columns WHERE table_schema = " + sqlLiteral(c.schema || "public") + " AND table_name = " + t + " ORDER BY ordinal_position";
  return "SELECT column_name AS name, data_type AS type, nullable, data_default AS default_value, '' AS key, '' AS extra FROM all_tab_columns WHERE owner = SYS_CONTEXT('USERENV','CURRENT_SCHEMA') AND table_name = " + t + " ORDER BY column_id";
}
function indexesQuery(c, table) {
  var t = sqlLiteral(table);
  if (c.driver === "sqlite") return "SELECT name, CASE WHEN \"unique\"=1 THEN 'YES' ELSE 'NO' END AS is_unique, origin FROM pragma_index_list(" + t + ")";
  if (c.driver === "mysql") return "SELECT index_name AS name, CASE WHEN non_unique=0 THEN 'YES' ELSE 'NO' END AS is_unique, column_name AS column_name FROM information_schema.statistics WHERE table_schema = DATABASE() AND table_name = " + t + " ORDER BY index_name, seq_in_index";
  if (c.driver === "postgres") return "SELECT indexname AS name, indexdef AS definition FROM pg_indexes WHERE schemaname = " + sqlLiteral(c.schema || "public") + " AND tablename = " + t;
  return "SELECT index_name AS name, uniqueness AS is_unique FROM all_indexes WHERE table_owner = SYS_CONTEXT('USERENV','CURRENT_SCHEMA') AND table_name = " + t;
}
function rowsToObjects(result) {
  var out = [];
  if (!result) return out;
  var cols = result.columns || [];
  for (var i = 0; i < (result.rows || []).length; i++) {
    var o = {};
    for (var j = 0; j < cols.length; j++) o[cols[j].toLowerCase()] = result.rows[i][j];
    out.push(o);
  }
  return out;
}
function loadTables() {
  var c = selectedConnection();
  if (!c) return;
  state.schemaLoading = true;
  state.error = "";
  state.status = "正在读取表结构…";
  state.statusTone = "accent";
  callDB("query", tablesQuery(c), function (result) {
    state.tables = rowsToObjects(result);
    state.tablesFor = c.id;
    state.schemaLoading = false;
    state.status = "已加载 " + state.tables.length + " 张表";
    state.statusTone = "ok";
    nemorous.invalidate();
  }, function (e) { state.schemaLoading = false; setError(errText(e)); });
}
function selectTable(name) {
  state.selectedTable = name;
  state.columns = [];
  state.indexes = [];
  state.schemaSeq++;
  loadTable(name, state.schemaSeq);
}
function loadTable(name, seq) {
  var c = selectedConnection();
  if (!c || !name) return;
  var token = seq || ++state.schemaSeq;
  state.schemaLoading = true;
  callDB("query", columnsQuery(c, name), function (result) {
    if (token !== state.schemaSeq || name !== state.selectedTable) return;
    state.columns = rowsToObjects(result);
    nemorous.invalidate();
    callDB("query", indexesQuery(c, name), function (idx) {
      if (token !== state.schemaSeq || name !== state.selectedTable) return;
      state.indexes = rowsToObjects(idx);
      state.schemaLoading = false;
      nemorous.invalidate();
    }, function () {
      if (token !== state.schemaSeq || name !== state.selectedTable) return;
      state.indexes = [];
      state.schemaLoading = false;
      nemorous.invalidate();
    });
  }, function (e) {
    if (token !== state.schemaSeq || name !== state.selectedTable) return;
    state.schemaLoading = false; setError(errText(e));
  });
}
function previewTable() {
  if (!state.selectedTable) return;
  var c = selectedConnection();
  if (!c) return;
  var name = quoteIdent(state.selectedTable);
  var limit = c.driver === "oracle" ? "FETCH FIRST 100 ROWS ONLY" : "LIMIT 100";
  state.sql = "SELECT * FROM " + name + (c.driver === "oracle" ? " " + limit : " " + limit);
  state.view = "query";
  runQuery();
}

// ---------- connection management ----------
function editConnection(c) {
  state.editing = c ? c.id : "";
  state.form = c ? {
    id: c.id, name: c.name || "", driver: c.driver || "sqlite", host: c.host || "",
    port: c.port || "", database: c.database || "", user: c.user || "",
    password: c.password || "", path: c.path || "", schema: c.schema || "",
    sslmode: c.sslmode || "disable", dsn: c.dsn || ""
  } : emptyForm();
  state.formKey++;
  state.view = "connections";
}
function saveConnection() {
  var f = state.form;
  if (!f.name.trim()) { setError("请填写连接名称"); return; }
  if (f.driver === "sqlite" && !f.path.trim()) { setError("请填写 SQLite 文件路径"); return; }
  if (f.driver !== "sqlite" && !f.dsn.trim() && (!f.host.trim() || !f.user.trim())) { setError("请填写主机与用户名，或提供高级 DSN"); return; }
  var existing = null;
  if (f.id) existing = findConnection(f.id);
  var c = {
    id: f.id || "conn-" + Date.now() + "-" + Math.floor(Math.random() * 10000),
    name: f.name.trim(), driver: f.driver, host: f.host.trim(), port: f.port.trim() || defaultPort(f.driver),
    database: f.database.trim(), user: f.user.trim(), password: f.password,
    path: f.path.trim(), schema: f.schema.trim(), sslmode: f.sslmode || "disable", dsn: f.dsn.trim()
  };
  if (existing) {
    for (var i = 0; i < state.connections.length; i++) if (state.connections[i].id === existing.id) state.connections[i] = c;
  } else {
    state.connections.push(c);
  }
  state.selected = c.id;
  state.editing = "";
  saveConnections();
  nemorous.storage.set("selected", c.id);
  state.view = "query";
  state.status = "已保存连接 " + c.name;
  state.statusTone = "ok";
  state.tables = [];
  state.selectedTable = "";
  nemorous.invalidate();
}
function deleteConnection(id) {
  var next = [];
  for (var i = 0; i < state.connections.length; i++) if (state.connections[i].id !== id) next.push(state.connections[i]);
  state.connections = next;
  if (state.selected === id) state.selected = next.length ? next[0].id : "";
  saveConnections();
  nemorous.storage.set("selected", state.selected);
  state.editing = "";
  state.view = state.connections.length ? "query" : "connections";
  nemorous.invalidate();
}
function testConnection() {
  var c = state.editing ? state.form : selectedConnection();
  if (!c) { setError("没有可测试的连接"); return; }
  var temp = c.id ? c : {
    id: "__test__", name: c.name, driver: c.driver, host: c.host, port: c.port,
    database: c.database, user: c.user, password: c.password, path: c.path, schema: c.schema,
    sslmode: c.sslmode, dsn: c.dsn
  };
  state.running = true;
  state.status = "正在测试连接…";
  state.statusTone = "accent";
  callDBFor(temp, "query", testSQL(c.driver), function () {
    state.running = false;
    state.status = "连接成功";
    state.statusTone = "ok";
    nemorous.invalidate();
  }, function (e) {
    state.running = false;
    setError("连接失败：" + errText(e));
  });
}

// ---------- UI ----------
function header() {
  var c = selectedConnection();
  return row([
    text("Database", { bold: true, size: 13 }),
    c ? badge(driverLabel(c.driver), "accent") : badge("未连接", "muted"),
    c ? text(c.name, { size: 10.5, muted: true }) : null,
    spacer()
  ], { gap: 5 });
}
function tabs() {
  return row([
    btn("查询", "view", { value: "query" }, { small: true, active: state.view === "query" }),
    btn("结构", "view", { value: "schema" }, { small: true, active: state.view === "schema" }),
    btn("连接", "openConnections", null, { small: true, active: state.view === "connections" })
  ], { gap: 2 });
}
function connectionSidebar() {
  var items = [];
  for (var i = 0; i < state.connections.length; i++) {
    var c = state.connections[i];
    items.push({
      text: c.name,
      detail: driverLabel(c.driver),
      action: "selectConnection",
      payload: { id: c.id },
      selected: c.id === state.selected,
      tone: c.id === state.selected ? "accent" : ""
    });
  }
  return col([
    text("连接", { bold: true, size: 10 }),
    items.length ? list(items, { grow: true, scroll: true }) : hint("还没有数据库连接。"),
    state.connections.length ? text(state.connections.length + " 个连接", { size: 9, muted: true }) : null
  ], { width: 190, heightPercent: 100, gap: 5 });
}
function queryView() {
  var c = selectedConnection();
  if (!c) return [card([text("添加数据库连接", { bold: true, size: 12 }), hint("支持 SQLite、MySQL、PostgreSQL 与 Oracle。"), btn("新建连接", "newConnection", null, { primary: true })], { padding: 12, gap: 6 })];
  var children = [];
  children.push(card([
    row([text("SQL", { bold: true, size: 10.5 }), badge(driverLabel(c.driver), "accent"), spacer(), btn("清空", "clearSQL", null, { small: true }), btn(state.running ? "执行中…" : "执行", "runQuery", null, { primary: true, small: true, disabled: state.running })], { gap: 4 }),
    el("textarea", { key: "db-sql:" + state.sqlKey, value: state.sql, action: "sqlInput", height: 130, grow: true }),
    hint("点击执行按钮运行 SQL；查询默认最多返回 2000 行。")
  ], { padding: 10, gap: 4 }));
  if (state.result) {
    var r = state.result;
    var resultChildren = [row([text("结果", { bold: true, size: 10.5 }), badge(r.rowCount + " 行", "ok"), badge(r.durationMs + " ms", "muted"), r.truncated ? badge("已截断", "warn") : null, spacer()], { gap: 4 })];
    resultChildren = resultChildren.concat(resultTable());
    children.push(card(resultChildren, { padding: 10, gap: 4 }));
  } else if (state.execResult) {
    children.push(card([
      sectionTitle("执行结果", badge("完成", "ok")),
      text("影响行数：" + state.execResult.rowsAffected + (state.execResult.lastInsertId ? " · Last Insert ID：" + state.execResult.lastInsertId : ""), { size: 10.5, mono: true }),
      text("耗时：" + state.execResult.durationMs + " ms", { size: 9.5, muted: true })
    ], { padding: 10, gap: 4 }));
  }
  if (state.history.length) {
    var items = [];
    for (var i = 0; i < state.history.length; i++) {
      var h = state.history[i];
      items.push({
        text: h.sql.replace(/\s+/g, " ").slice(0, 120),
        detail: (h.ok ? "成功" : "失败") + " · " + h.connection + " · " + h.at,
        tone: h.ok ? "muted" : "err",
        action: "history",
        payload: { sql: h.sql },
        mono: true
      });
    }
    children.push(card([sectionTitle("查询历史", badge(state.history.length + " 条", "muted")), list(items, { maxHeight: 130, scroll: true })], { padding: 10, gap: 4 }));
  }
  return children;
}
function schemaView() {
  var c = selectedConnection();
  if (!c) return [hint("请先选择连接。")];
  var children = [];
  var tableItems = [];
  for (var i = 0; i < state.tables.length; i++) {
    var t = state.tables[i];
    tableItems.push({
      text: t.name,
      detail: (t.type || "table").toLowerCase(),
      action: "selectTable",
      payload: { name: t.name },
      selected: t.name === state.selectedTable,
      mono: true
    });
  }
  children.push(card([
    row([sectionTitle("表 / 视图", badge(state.tables.length + " 张", "muted")), btn(state.schemaLoading ? "加载中…" : "刷新结构", "loadTables", null, { small: true, disabled: state.schemaLoading })], { gap: 4 }),
    tableItems.length ? list(tableItems, { maxHeight: 220, scroll: true }) : hint("点击「刷新结构」读取数据库表。")
  ], { padding: 10, gap: 4 }));
  if (state.selectedTable) {
    var colHeaders = ["Column", "Type", "Nullable", "Default", "Key", "Extra"];
    var rows = [];
    for (var j = 0; j < state.columns.length; j++) {
      var x = state.columns[j];
      rows.push([x.name || "", x.type || "", x.nullable || "", x.default_value == null ? "" : String(x.default_value), x.key || "", x.extra || ""]);
    }
    children.push(card([
      row([text("表 " + state.selectedTable, { bold: true, size: 11, mono: true }), badge(state.columns.length + " 列", "muted"), spacer(), btn("预览前 100 行", "previewTable", null, { small: true })], { gap: 4 }),
      rows.length ? el("table", { headers: colHeaders, rows: rows, maxHeight: 320, scroll: true }) : hint("正在读取列信息…")
    ], { padding: 10, gap: 5 }));
    if (state.indexes.length) {
      var ih = Object.keys(state.indexes[0]).map(function (k) { return k; });
      var ir = [];
      for (j = 0; j < state.indexes.length; j++) {
        var r = [];
        for (var k = 0; k < ih.length; k++) r.push(formatCell(state.indexes[j][ih[k]]));
        ir.push(r);
      }
      children.push(card([sectionTitle("索引", badge(state.indexes.length + " 个", "muted")), el("table", { headers: ih, rows: ir, maxHeight: 180, scroll: true })], { padding: 10, gap: 4 }));
    }
  }
  return children;
}
function connectionsView() {
  var children = [];
  var form = state.form;
  var editing = state.view === "connections";
  var listItems = [];
  for (var i = 0; i < state.connections.length; i++) {
    var c = state.connections[i];
    listItems.push({ text: c.name, detail: driverLabel(c.driver), action: "selectConnection", payload: { id: c.id }, selected: c.id === state.selected && !editing });
  }
  children.push(card([
    row([sectionTitle("数据库连接", badge(state.connections.length + " 个", "muted")), spacer(), btn("新建", "newConnection", null, { small: true, primary: true })], { gap: 4 }),
    listItems.length ? list(listItems, { maxHeight: 150, scroll: true }) : hint("还没有连接。")
  ], { padding: 10, gap: 4 }));
  if (editing) {
    var fields = [text(form.id ? "编辑连接" : "新建连接", { bold: true, size: 11.5 })];
    fields.push(row([
      btn("SQLite", "driver", { value: "sqlite" }, { small: true, active: form.driver === "sqlite" }),
      btn("MySQL", "driver", { value: "mysql" }, { small: true, active: form.driver === "mysql" }),
      btn("PostgreSQL", "driver", { value: "postgres" }, { small: true, active: form.driver === "postgres" }),
      btn("Oracle", "driver", { value: "oracle" }, { small: true, active: form.driver === "oracle" })
    ], { gap: 3 }));
    fields.push(connField("名称", "name", form.name, "例如 本地测试库"));
    if (form.driver === "sqlite") {
      fields.push(connField("文件路径", "path", form.path, "相对工作区路径或绝对路径"));
    } else {
      fields.push(row([
        col([text("主机", { muted: true, size: 9.5 }), connField("", "host", form.host, "db.example.com")], { grow: true, gap: 3 }),
        col([text("端口", { muted: true, size: 9.5 }), connField("", "port", form.port, defaultPort(form.driver), 100)], { width: 110, gap: 3 })
      ], { gap: 6 }));
      fields.push(row([
        col([text("数据库 / Service", { muted: true, size: 9.5 }), connField("", "database", form.database, "database")], { grow: true, gap: 3 }),
        col([text("Schema", { muted: true, size: 9.5 }), connField("", "schema", form.schema, form.driver === "postgres" ? "public" : "")], { width: 180, gap: 3 })
      ], { gap: 6 }));
      fields.push(row([
        col([text("用户名", { muted: true, size: 9.5 }), connField("", "user", form.user, "user")], { grow: true, gap: 3 }),
        col([text("密码", { muted: true, size: 9.5 }), el("input", { key: "conn:password:" + state.formKey, value: form.password, placeholder: "password", action: "connField", password: true, grow: true })], { grow: true, gap: 3 })
      ], { gap: 6 }));
      if (form.driver === "postgres") fields.push(connField("SSL Mode", "sslmode", form.sslmode, "disable / require"));
      fields.push(connField("高级 DSN（可选）", "dsn", form.dsn, "覆盖自动生成的连接串"));
    }
    fields.push(row([
      btn("保存连接", "saveConnection", null, { primary: true }),
      btn("测试连接", "testConnection", null, { small: true, disabled: state.running }),
      form.id ? btn("删除", "deleteConnection", { id: form.id }, { small: true }) : null,
      spacer()
    ], { gap: 4 }));
    children.push(card(fields, { padding: 10, gap: 5 }));
  }
  children.push(card([
    text("说明", { bold: true, size: 10.5 }),
    hint("SQLite 支持工作区相对路径；MySQL / PostgreSQL / Oracle 需填写主机、库名与账号。"),
    hint("连接信息保存在本地插件存储，不会写入工作区。数据库驱动为纯 Go，跨平台无需安装客户端。")
  ], { padding: 10, gap: 4 }));
  return children;
}
function connField(label, key, value, placeholder, width) {
  if (label) {
    return row([text(label, { muted: true, size: 9.5, width: 90 }), input("conn:" + key + ":" + state.formKey, value, placeholder, { action: "connField", grow: true })], { gap: 4 });
  }
  return input("conn:" + key + ":" + state.formKey, value, placeholder, { action: "connField", grow: true, width: width || 0 });
}

// ---------- actions ----------
function handleAction(action, payload, panelID) {
  var p = payload || {};
  switch (action) {
    case "open":
    case "openQuery":
      state.view = "query";
      break;
    case "openConnections":
      state.view = "connections";
      if (!state.editing) editConnection(selectedConnection() || null);
      break;
    case "view":
      state.view = p.value || "query";
      if (state.view === "schema" && state.selected && state.tablesFor !== state.selected) loadTables();
      break;
    case "selectConnection":
      state.selected = p.id || "";
      nemorous.storage.set("selected", state.selected);
      state.tables = []; state.selectedTable = ""; state.columns = []; state.indexes = []; state.schemaSeq++;
      state.result = null; state.execResult = null;
      if (state.view === "connections") editConnection(selectedConnection());
      else state.view = "query";
      state.status = "已切换连接";
      state.statusTone = "muted";
      break;
    case "newConnection":
      editConnection(null);
      return;
    case "driver":
      state.form.driver = p.value || "sqlite";
      if (!state.form.port) state.form.port = defaultPort(state.form.driver);
      break;
    case "connField":
      var key = String(p.key || "");
      var parts = key.split(":");
      var field = parts.length > 1 ? parts[1] : "";
      if (field === "name") state.form.name = String(p.value || "");
      else if (field === "host") state.form.host = String(p.value || "");
      else if (field === "port") state.form.port = String(p.value || "");
      else if (field === "database") state.form.database = String(p.value || "");
      else if (field === "user") state.form.user = String(p.value || "");
      else if (field === "password") state.form.password = String(p.value == null ? "" : p.value);
      else if (field === "path") state.form.path = String(p.value || "");
      else if (field === "schema") state.form.schema = String(p.value || "");
      else if (field === "sslmode") state.form.sslmode = String(p.value || "");
      else if (field === "dsn") state.form.dsn = String(p.value || "");
      break;
    case "saveConnection":
      saveConnection();
      return;
    case "testConnection":
      testConnection();
      return;
    case "deleteConnection":
      deleteConnection(p.id || state.form.id);
      return;
    case "sqlInput":
      state.sql = String(p.value == null ? "" : p.value);
      nemorous.storage.set("sql", state.sql);
      return;
    case "runQuery":
      runQuery();
      return;
    case "clearSQL":
      state.sql = ""; state.sqlKey++; state.result = null; state.execResult = null;
      break;
    case "loadTables":
      loadTables(); return;
    case "selectTable":
      selectTable(p.name || ""); return;
    case "previewTable":
      previewTable(); return;
    case "history":
      state.sql = p.sql || ""; state.sqlKey++; state.view = "query";
      break;
    default:
      return;
  }
  nemorous.invalidate();
}

function render(panelID) {
  var children = [header(), tabs()];
  if (state.running || state.schemaLoading) children.push({ type: "spinner", text: state.status || "正在执行…" });
  else if (state.status) children.push(text(state.status, { size: 9.5, muted: true, tone: state.statusTone || "" }));
  if (state.error) children.push(text(state.error, { tone: "err", size: 9.5 }));
  var content = state.view === "connections" ? connectionsView() : state.view === "schema" ? schemaView() : queryView();
  if (state.view === "connections") {
    for (var i = 0; i < content.length; i++) children.push(content[i]);
  } else {
    var body = col(content, { grow: true, gap: 6, scroll: true });
    children.push(row([
      connectionSidebar(),
      { type: "divider", vertical: true },
      col([body], { grow: true, heightPercent: 100 })
    ], { grow: true, gap: 6 }));
  }
  return col(children, { grow: true, padding: 8, gap: 6 });
}
