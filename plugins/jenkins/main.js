// Jenkins tool window: classic JSON API client with session-cookie CSRF support.
// It intentionally sticks to stable /api/json endpoints so old and new
// Jenkins installations behave the same way.

var MAX_CONSOLE = 220000;

var state = {
  view: "jobs",
  settings: {
    baseUrl: "http://localhost:8080/",
    username: "",
    password: "",
    insecure: false,
    autoRefresh: true
  },
  settingsKey: 0,
  jobs: [],
  filter: "",
  onlyAssociated: false,
  selected: "",
  job: null,
  builds: [],
  build: null,
  buildURL: "",
  console: "",
  loading: false,
  status: "",
  statusTone: "",
  error: "",
  jenkinsVersion: "",
  crumb: { field: "", value: "" },
  cookies: {},
  queue: null,
  pollTimer: 0,
  pollCount: 0,
  paramValues: {},
  associations: {},
  repos: [],
  connected: false,
  seq: 0
};

function activate() {
  var s = nemorous.storage.get("settings");
  if (s && typeof s === "object") {
    state.settings.baseUrl = s.baseUrl || state.settings.baseUrl;
    state.settings.username = s.username || "";
    state.settings.password = s.password || "";
    state.settings.insecure = !!s.insecure;
    state.settings.autoRefresh = s.autoRefresh !== false;
  }
  var a = nemorous.storage.get("associations");
  if (a && typeof a === "object") state.associations = a;
  syncRepos();
}

function onWorkspaceChanged() {
  syncRepos();
  state.jobs = [];
  state.job = null;
  state.build = null;
  state.console = "";
  state.selected = "";
  state.connected = false;
}

function saveSettings() {
  nemorous.storage.set("settings", state.settings);
}

function saveAssociations() {
  nemorous.storage.set("associations", state.associations);
}

function syncRepos() {
  var ws = nemorous.workspace();
  state.repos = ws.repos || [];
  if (!state.repos.length && ws.path) {
    state.repos = [{ id: "workspace", name: baseName(ws.path) || "workspace", path: ws.path }];
  }
}

// ---------- small helpers ----------
function el(type, props) {
  var n = props || {};
  n.type = type;
  return n;
}
function text(value, props) {
  var n = props || {};
  n.type = "text";
  n.text = String(value == null ? "" : value);
  return n;
}
function hint(value) { return text(value, { muted: true, size: 10.5 }); }
function badge(value, tone) { return el("badge", { text: String(value), tone: tone || "" }); }
function btn(label, action, payload, props) {
  var n = props || {};
  n.type = "button";
  n.text = label;
  n.action = action;
  if (payload !== null && payload !== undefined) n.payload = payload;
  return n;
}
function input(key, value, placeholder, props) {
  var n = props || {};
  n.type = "input";
  n.key = key;
  n.value = value == null ? "" : String(value);
  n.placeholder = placeholder || "";
  n.action = n.action || "input";
  return n;
}
function code(value, props) {
  var n = props || {};
  n.type = "code";
  n.text = String(value || "");
  n.mono = true;
  return n;
}
function row(children, props) {
  var n = props || {};
  n.type = "row";
  n.children = clean(children);
  return n;
}
function col(children, props) {
  var n = props || {};
  n.type = "column";
  n.children = clean(children);
  return n;
}
function list(items, props) {
  var n = props || {};
  n.type = "list";
  n.items = items;
  return n;
}
function clean(children) {
  var out = [];
  for (var i = 0; i < (children || []).length; i++) if (children[i]) out.push(children[i]);
  return out;
}
function spacer() { return { type: "spacer" }; }
function divider() { return { type: "divider" }; }
function baseName(p) {
  if (!p) return "";
  var v = String(p).replace(/[\\/]+$/, "");
  var i = Math.max(v.lastIndexOf("/"), v.lastIndexOf("\\"));
  return i >= 0 ? v.substring(i + 1) : v;
}
function shortClass(cls) {
  var v = String(cls || "");
  var i = v.lastIndexOf(".");
  return i >= 0 ? v.substring(i + 1) : v;
}
function jobType(cls) {
  var c = shortClass(cls);
  var map = {
    FreeStyleProject: "自由风格",
    WorkflowJob: "Pipeline",
    WorkflowMultiBranchProject: "多分支 Pipeline",
    WorkflowRun: "Pipeline 构建",
    MavenModuleSet: "Maven",
    MatrixProject: "矩阵任务",
    Folder: "文件夹",
    OrganizationFolder: "组织文件夹",
    MultiJobProject: "MultiJob",
    IvyModuleSet: "Ivy",
    Project: "任务",
    Job: "任务"
  };
  return map[c] || c || "任务";
}
function isFolderClass(cls) {
  var c = shortClass(cls);
  return c === "Folder" || c === "OrganizationFolder" || c === "WorkflowMultiBranchProject";
}
function ensureSlash(url) {
  var v = String(url || "");
  return /\/$/.test(v) ? v : v + "/";
}
function baseURL() {
  return String(state.settings.baseUrl || "").replace(/\/+$/, "");
}
function nowText(ms) {
  if (!ms) return "";
  try { return new Date(ms).toLocaleString(); } catch (e) { return ""; }
}
function durationText(ms) {
  var n = Number(ms || 0);
  if (!n) return "—";
  if (n < 1000) return n + "ms";
  var s = Math.round(n / 1000);
  if (s < 60) return s + "s";
  var m = Math.floor(s / 60);
  s = s % 60;
  if (m < 60) return m + "m " + s + "s";
  return Math.floor(m / 60) + "h " + (m % 60) + "m";
}
function resultLabel(result, building) {
  if (building) return "运行中";
  if (!result) return "未完成";
  if (result === "SUCCESS") return "成功";
  if (result === "FAILURE") return "失败";
  if (result === "UNSTABLE") return "不稳定";
  if (result === "ABORTED") return "已中止";
  if (result === "NOT_BUILT") return "未构建";
  return result;
}
function jobStatus(job) {
  var color = String((job && job.color) || "");
  if (/_anime$/.test(color)) return { text: "运行中", tone: "accent", building: true };
  if (color === "red") return { text: "失败", tone: "err", building: false };
  if (color === "yellow") return { text: "不稳定", tone: "warn", building: false };
  if (color === "blue") return { text: "成功", tone: "ok", building: false };
  if (color === "disabled") return { text: "已禁用", tone: "muted", building: false };
  if (color === "aborted") return { text: "已中止", tone: "muted", building: false };
  if (color === "notbuilt" || color === "grey") return { text: "未构建", tone: "muted", building: false };
  return { text: "未知", tone: "muted", building: false };
}
function buildTone(result, building) {
  if (building) return "accent";
  if (result === "SUCCESS") return "ok";
  if (result === "FAILURE") return "err";
  if (result === "UNSTABLE") return "warn";
  return "muted";
}
function trimConsole(s) {
  s = String(s || "");
  return s.length > MAX_CONSOLE ? s.substring(s.length - MAX_CONSOLE) : s;
}
function headerValue(headers, name) {
  if (!headers) return "";
  var low = String(name).toLowerCase();
  for (var k in headers) {
    if (headers.hasOwnProperty(k) && k.toLowerCase() === low) return String(headers[k] || "");
  }
  return "";
}
function errorText(resp) {
  var body = String((resp && resp.body) || "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
  if (body.length > 400) body = body.slice(0, 400);
  return "HTTP " + resp.status + (body ? " · " + body : "");
}
function b64(input) {
  var chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  var str = "";
  try { str = unescape(encodeURIComponent(String(input))); } catch (e) { str = String(input); }
  var out = "";
  for (var i = 0; i < str.length; i += 3) {
    var a = str.charCodeAt(i);
    var b = i + 1 < str.length ? str.charCodeAt(i + 1) : NaN;
    var c = i + 2 < str.length ? str.charCodeAt(i + 2) : NaN;
    var n = (a << 16) | ((isNaN(b) ? 0 : b) << 8) | (isNaN(c) ? 0 : c);
    out += chars.charAt((n >> 18) & 63) + chars.charAt((n >> 12) & 63) +
      (isNaN(b) ? "=" : chars.charAt((n >> 6) & 63)) +
      (isNaN(c) ? "=" : chars.charAt(n & 63));
  }
  return out;
}

// ---------- HTTP / session ----------
function captureCookies(resp) {
  var raw = headerValue(resp.headers, "Set-Cookie");
  if (!raw) return;
  var parts = raw.split(/,(?=\s*[A-Za-z0-9_.-]+\s*=)/);
  for (var i = 0; i < parts.length; i++) {
    var m = parts[i].match(/^\s*([A-Za-z0-9_.-]+)=([^;]*)/);
    if (!m) continue;
    var name = m[1];
    if (name === "Path" || name === "Expires" || name === "Max-Age" || name === "HttpOnly") continue;
    state.cookies[name] = m[2];
  }
}
function cookieHeader() {
  var parts = [];
  for (var k in state.cookies) if (state.cookies.hasOwnProperty(k)) parts.push(k + "=" + state.cookies[k]);
  return parts.join("; ");
}
function requestHeaders(extra) {
  var h = {};
  if (extra) for (var k in extra) if (extra.hasOwnProperty(k)) h[k] = extra[k];
  if (state.settings.username || state.settings.password) {
    h.Authorization = "Basic " + b64((state.settings.username || "") + ":" + (state.settings.password || ""));
  }
  var c = cookieHeader();
  if (c) h.Cookie = c;
  return h;
}
function captureMeta(resp) {
  captureCookies(resp);
  var version = headerValue(resp.headers, "X-Jenkins");
  if (version) state.jenkinsVersion = version;
}
function request(method, url, body, headers, onOK, onFail, quiet) {
  if (!quiet) {
    state.loading = true;
    if (!state.status) state.status = "正在连接 Jenkins…";
  }
  var completed = false;
  try {
    nemorous.http.request({
      method: method,
      url: url,
      headers: requestHeaders(headers),
      body: body || "",
      timeoutMs: 25000,
      insecure: !!state.settings.insecure,
      onResponse: function (resp) {
        if (completed) return;
        completed = true;
        captureMeta(resp);
        if (!quiet) state.loading = false;
        if (!resp.ok) {
          if (onFail) onFail(resp);
          else {
            state.error = errorText(resp);
            state.status = "请求失败";
            state.statusTone = "err";
          }
          nemorous.invalidate();
          return;
        }
        if (onOK) onOK(resp);
        nemorous.invalidate();
      },
      onError: function (message) {
        if (completed) return;
        completed = true;
        if (!quiet) state.loading = false;
        if (onFail) onFail({ status: 0, body: message, headers: {} });
        else {
          state.error = "连接失败：" + String(message);
          state.status = "连接失败";
          state.statusTone = "err";
        }
        nemorous.invalidate();
      }
    });
  } catch (e) {
    if (!quiet) state.loading = false;
    state.error = String(e);
    state.status = "请求无法发送";
    state.statusTone = "err";
    nemorous.invalidate();
  }
}
function jsonRequest(url, onOK, onFail, quiet) {
  request("GET", url, "", null, function (resp) {
    var data;
    try { data = JSON.parse(resp.body); } catch (e) {
      state.error = "Jenkins 返回了无效 JSON";
      state.status = "解析失败";
      state.statusTone = "err";
      if (onFail) onFail(resp);
      return;
    }
    onOK(data, resp);
  }, onFail, quiet);
}
function ensureCrumb(cb) {
  if (state.crumb.value) { cb(); return; }
  var url = baseURL() + "/crumbIssuer/api/json";
  jsonRequest(url, function (data) {
    state.crumb.field = data && data.crumbRequestField ? data.crumbRequestField : "Jenkins-Crumb";
    state.crumb.value = data && data.crumb ? data.crumb : "";
    cb();
  }, function (resp) {
    if (resp.status === 404 || resp.status === 405) {
      state.crumb = { field: "", value: "" };
      cb();
      return;
    }
    state.error = "无法获取 CSRF Crumb：" + errorText(resp);
    state.status = "连接失败";
    state.statusTone = "err";
  }, true);
}
function postRequest(url, body, contentType, onOK, retried) {
  ensureCrumb(function () {
    var headers = {};
    if (contentType) headers["Content-Type"] = contentType;
    if (state.crumb.field && state.crumb.value) headers[state.crumb.field] = state.crumb.value;
    request("POST", url, body, headers, function (resp) {
      onOK(resp);
    }, function (resp) {
      if (resp.status === 403 && !retried) {
        state.crumb = { field: "", value: "" };
        postRequest(url, body, contentType, onOK, true);
        return;
      }
      state.error = errorText(resp);
      state.status = "构建请求失败";
      state.statusTone = "err";
    });
  });
}

// ---------- jobs ----------
function flattenJobs(items, prefix, out) {
  var list = items || [];
  for (var i = 0; i < list.length; i++) {
    var j = list[i];
    if (!j || !j.name) continue;
    var full = prefix ? prefix + "/" + j.name : j.name;
    if (j.jobs && j.jobs.length) {
      flattenJobs(j.jobs, full, out);
      continue;
    }
    if (isFolderClass(j._class)) continue;
    var cls = shortClass(j._class);
    out.push({
      name: j.name,
      fullName: full,
      url: j.url || "",
      color: j.color || "",
      cls: cls,
      type: jobType(j._class),
      typeRaw: j._class || "",
      associated: repoForJob(j.url || "")
    });
  }
  return out;
}
function repoForJob(url) {
  var id = state.associations[ensureSlash(url)];
  if (!id) return null;
  for (var i = 0; i < state.repos.length; i++) if (state.repos[i].id === id) return state.repos[i];
  return { id: id, name: id, path: "" };
}
function jobCounts() {
  var out = { all: state.jobs.length, ok: 0, fail: 0, running: 0, associated: 0 };
  for (var i = 0; i < state.jobs.length; i++) {
    var st = jobStatus(state.jobs[i]);
    if (st.tone === "ok") out.ok++;
    if (st.tone === "err") out.fail++;
    if (st.building) out.running++;
    if (state.associations[ensureSlash(state.jobs[i].url)]) out.associated++;
  }
  return out;
}
function connect() {
  var base = baseURL();
  if (!base) { state.error = "请填写 Jenkins 地址"; state.view = "settings"; return; }
  state.error = "";
  state.status = "正在读取 Jenkins 任务…";
  state.statusTone = "accent";
  state.jobs = [];
  state.job = null;
  state.build = null;
  state.console = "";
  state.crumb = { field: "", value: "" };
  state.cookies = {};
  var tree = "jobs[name,url,color,_class,jobs[name,url,color,_class,jobs[name,url,color,_class,jobs[name,url,color,_class]]]]";
  jsonRequest(base + "/api/json?tree=" + encodeURIComponent(tree), function (data) {
    var flat = flattenJobs(data.jobs || [], "", []);
    state.jobs = flat;
    state.connected = true;
    state.status = "已连接 · " + flat.length + " 个任务";
    state.statusTone = "ok";
    state.view = "jobs";
    if (!state.selected || !findJob(state.selected)) {
      state.selected = flat.length ? flat[0].url : "";
    }
    if (state.selected) loadJob(state.selected);
  }, function (resp) {
    state.connected = false;
    state.status = "连接失败";
    state.statusTone = "err";
    if (resp.status === 401) state.error = "认证失败，请检查用户名与密码 / API Token";
    else if (resp.status === 403) state.error = "没有权限读取 Jenkins，请检查用户权限";
  });
}
function findJob(url) {
  var target = ensureSlash(url);
  for (var i = 0; i < state.jobs.length; i++) if (ensureSlash(state.jobs[i].url) === target) return state.jobs[i];
  return null;
}
function selectedJob() {
  return findJob(state.selected);
}
function jobDetailTree() {
  return "name,url,color,_class,description,buildable,inQueue,lastBuild[number,url,result,timestamp,duration,displayName],lastCompletedBuild[number,url,result,timestamp,duration,displayName],lastSuccessfulBuild[number],lastFailedBuild[number],healthReport[description,score,iconClassName],property[parameterDefinitions[name,type,description,defaultParameterValue[value],choices]],builds[number,url,result,timestamp,duration,displayName]{0,25}";
}
function loadJob(url) {
  if (!url) return;
  stopPolling();
  state.selected = url;
  state.build = null;
  state.buildURL = "";
  state.console = "";
  state.status = "正在读取任务详情…";
  state.statusTone = "accent";
  jsonRequest(ensureSlash(url) + "api/json?tree=" + encodeURIComponent(jobDetailTree()), function (data) {
    state.job = data;
    state.builds = data.builds || [];
    if (!state.builds.length && data.lastBuild) state.builds = [data.lastBuild];
    state.selected = data.url || url;
    state.status = "任务详情已加载";
    state.statusTone = "ok";
    initParams();
    refreshJobInList(data);
    if (state.builds.length && !state.build) loadBuild(state.builds[0].url);
  }, function (resp) {
    state.error = "读取任务详情失败：" + errorText(resp);
    state.status = "读取失败";
    state.statusTone = "err";
  });
}
function refreshJobInList(detail) {
  for (var i = 0; i < state.jobs.length; i++) {
    if (ensureSlash(state.jobs[i].url) === ensureSlash(detail.url)) {
      state.jobs[i].color = detail.color || state.jobs[i].color;
      break;
    }
  }
}
function extractParams(detail) {
  var params = [];
  var props = (detail && detail.property) || [];
  for (var i = 0; i < props.length; i++) {
    var defs = props[i].parameterDefinitions || [];
    for (var j = 0; j < defs.length; j++) {
      var d = defs[j] || {};
      var type = shortClass(d.type);
      var dv = d.defaultParameterValue ? d.defaultParameterValue.value : "";
      if (dv !== null && typeof dv === "object") {
        if (dv.length !== undefined) dv = dv.join(",");
        else dv = JSON.stringify(dv);
      }
      params.push({
        name: d.name || "",
        type: type,
        description: d.description || "",
        defaultValue: dv == null ? "" : String(dv),
        choices: d.choices || []
      });
    }
  }
  return params;
}
function paramKey(name) {
  return ensureSlash(state.selected) + "|" + name;
}
function initParams() {
  var params = extractParams(state.job);
  for (var i = 0; i < params.length; i++) {
    var key = paramKey(params[i].name);
    if (state.paramValues[key] === undefined) state.paramValues[key] = params[i].defaultValue;
  }
}
function selectedParam(name) {
  var key = paramKey(name);
  return state.paramValues[key];
}
function setParam(name, value) {
  state.paramValues[paramKey(name)] = String(value == null ? "" : value);
}

// ---------- builds ----------
function loadBuild(url) {
  if (!url) return;
  stopPolling();
  state.buildURL = url;
  state.status = "正在读取构建详情…";
  state.statusTone = "accent";
  jsonRequest(ensureSlash(url) + "api/json?tree=number,url,result,building,duration,estimatedDuration,timestamp,displayName,fullDisplayName,description,builtOn,actions[parameters[name,value]],changeSet[items[commitId,msg,author[fullName]]]", function (data) {
    state.build = data;
    state.status = "构建 #" + data.number + " · " + resultLabel(data.result, data.building);
    state.statusTone = buildTone(data.result, data.building);
    loadConsole(data.building);
    if (data.building && state.settings.autoRefresh) scheduleBuildPoll(ensureSlash(data.url));
  }, function (resp) {
    state.error = "读取构建失败：" + errorText(resp);
    state.status = "读取失败";
    state.statusTone = "err";
  });
}
function loadConsole(building) {
  if (!state.buildURL) return;
  request("GET", ensureSlash(state.buildURL) + "consoleText", "", null, function (resp) {
    state.console = trimConsole(resp.body || "");
    if (!building) state.loading = false;
  }, function (resp) {
    state.error = "读取控制台失败：" + errorText(resp);
  }, true);
}
function scheduleBuildPoll(url) {
  stopPolling();
  state.pollCount++;
  if (state.pollCount > 180) {
    state.status = "构建仍在运行，已停止自动轮询";
    state.statusTone = "warn";
    return;
  }
  state.pollTimer = setTimeout(function () {
    jsonRequest(ensureSlash(url) + "api/json?tree=number,url,result,building,duration,timestamp,displayName,fullDisplayName", function (data) {
      state.build = data;
      state.status = "构建 #" + data.number + " · " + resultLabel(data.result, data.building);
      state.statusTone = buildTone(data.result, data.building);
      loadConsole(data.building);
      if (data.building && state.settings.autoRefresh) scheduleBuildPoll(url);
      else {
        state.pollCount = 0;
        if (state.job) loadJob(state.job.url);
      }
    }, function () {}, true);
  }, 2000);
}
function stopPolling() {
  if (state.pollTimer) {
    clearTimeout(state.pollTimer);
    state.pollTimer = 0;
  }
}
function triggerBuild() {
  var job = state.job;
  if (!job || job.buildable === false) {
    state.error = "当前任务不可构建";
    return;
  }
  var params = extractParams(job);
  var body = "";
  if (params.length) {
    var parts = [];
    for (var i = 0; i < params.length; i++) {
      var p = params[i];
      var v = selectedParam(p.name);
      if (v === undefined || v === null) v = p.defaultValue;
      if (p.type === "BooleanParameterDefinition") v = String(v) === "true" ? "true" : "false";
      parts.push(encodeURIComponent(p.name) + "=" + encodeURIComponent(v));
    }
    body = parts.join("&");
  }
  var endpoint = ensureSlash(job.url) + (params.length ? "buildWithParameters" : "build");
  stopPolling();
  state.error = "";
  state.status = "正在触发构建…";
  state.statusTone = "accent";
  postRequest(endpoint, body, "application/x-www-form-urlencoded", function (resp) {
    var loc = headerValue(resp.headers, "Location");
    state.status = "构建已进入队列";
    state.statusTone = "accent";
    if (loc) {
      state.queue = { url: ensureSlash(loc), count: 0 };
      pollQueue();
    } else {
      setTimeout(function () { loadJob(job.url); }, 1200);
    }
  });
}
function pollQueue() {
  if (!state.queue) return;
  var q = state.queue;
  q.count++;
  jsonRequest(ensureSlash(q.url) + "api/json?tree=id,why,executable[number,url],cancelled", function (data) {
    if (data.cancelled) {
      state.queue = null;
      state.status = "构建已取消";
      state.statusTone = "err";
      return;
    }
    if (data.executable && data.executable.url) {
      state.queue = null;
      state.status = "开始构建 #" + data.executable.number;
      state.statusTone = "accent";
      loadBuild(data.executable.url);
      if (state.job) loadJob(state.job.url);
      return;
    }
    if (q.count > 60) {
      state.queue = null;
      state.status = "排队超时，请手动刷新";
      state.statusTone = "warn";
      return;
    }
    state.status = "排队中" + (data.why ? " · " + data.why : "…");
    state.statusTone = "accent";
    state.pollTimer = setTimeout(pollQueue, 1200);
  }, function (resp) {
    if (resp.status === 404) {
      state.queue = null;
      setTimeout(function () { if (state.job) loadJob(state.job.url); }, 800);
      return;
    }
    state.queue = null;
    state.status = "查询队列失败";
    state.statusTone = "err";
  }, true);
}
function buildHistory() {
  var out = [];
  var builds = state.builds || [];
  for (var i = 0; i < builds.length && i < 25; i++) {
    var b = builds[i];
    var label = b.displayName || "";
    if (!label) label = "#" + b.number;
    else if (label.indexOf("#") !== 0) label = "#" + b.number + " " + label;
    out.push({
      text: label,
      detail: resultLabel(b.result, false) + " · " + nowText(b.timestamp) + " · " + durationText(b.duration),
      tone: buildTone(b.result, false),
      action: "selectBuild",
      payload: { url: b.url },
      selected: state.build && state.build.number === b.number,
      mono: true
    });
  }
  return out;
}
function selectedBuildData() {
  return state.build;
}

// ---------- UI ----------
function header() {
  var counts = jobCounts();
  return row([
    text("Jenkins", { bold: true, size: 13 }),
    badge(state.connected ? "已连接" : "未连接", state.connected ? "ok" : "muted"),
    state.jenkinsVersion ? badge("Jenkins " + state.jenkinsVersion, "muted") : null,
    badge(counts.all + " 任务", "muted"),
    counts.fail ? badge(counts.fail + " 失败", "err") : null,
    counts.running ? badge(counts.running + " 运行中", "accent") : null,
    spacer(),
    btn("刷新", "refresh", null, { small: true, disabled: state.loading }),
    btn("设置", "settings", null, { small: true, active: state.view === "settings" })
  ], { gap: 5 });
}
function tabs() {
  return row([
    btn("任务", "view", { value: "jobs" }, { small: true, active: state.view === "jobs" }),
    btn("设置", "view", { value: "settings" }, { small: true, active: state.view === "settings" })
  ], { gap: 2 });
}
function summary() {
  var c = jobCounts();
  return row([
    badge(c.all + " 全部", "muted"),
    badge(c.ok + " 成功", "ok"),
    badge(c.fail + " 失败", c.fail ? "err" : "muted"),
    badge(c.running + " 运行中", c.running ? "accent" : "muted"),
    badge(c.associated + " 已关联仓库", c.associated ? "accent" : "muted")
  ], { gap: 4 });
}
function jobsView(children) {
  if (!state.repos.length) children.push(hint("未发现工作区仓库，仍可浏览 Jenkins 任务。"));
  children.push(summary());
  var jobs = filteredJobs();
  var left = col([
    input("filter", state.filter, "搜索任务 / 类型 / 仓库", { action: "filter" }),
    row([
      btn(state.onlyAssociated ? "仅关联 ✓" : "仅关联仓库", "toggleAssociated", null, { small: true, active: state.onlyAssociated }),
      spacer(),
      text(jobs.length + " 个任务", { muted: true, size: 9.5 })
    ], { gap: 4 }),
    jobList(jobs)
  ], { width: 300, heightPercent: 100, gap: 5 });
  var right = col(jobDetail(), { grow: true, heightPercent: 100, gap: 6 });
  children.push(row([left, { type: "divider", vertical: true }, right], { grow: true, gap: 6 }));
}
function filteredJobs() {
  var q = String(state.filter || "").toLowerCase();
  var out = [];
  for (var i = 0; i < state.jobs.length; i++) {
    var j = state.jobs[i];
    var assoc = state.associations[ensureSlash(j.url)];
    if (state.onlyAssociated && !assoc) continue;
    var hay = (j.name + " " + j.fullName + " " + j.type + " " + (assoc || "")).toLowerCase();
    if (q && hay.indexOf(q) < 0) continue;
    out.push(j);
  }
  return out;
}
function jobList(jobs) {
  var items = [];
  for (var i = 0; i < jobs.length; i++) {
    var j = jobs[i];
    var st = jobStatus(j);
    var repo = repoForJob(j.url);
    items.push({
      text: j.fullName || j.name,
      detail: j.type + (repo ? " · " + repo.name : "") + " · " + st.text,
      tone: st.tone,
      action: "selectJob",
      payload: { url: j.url },
      selected: ensureSlash(j.url) === ensureSlash(state.selected),
      mono: false
    });
  }
  return list(items, { grow: true, scroll: true });
}
function jobDetail() {
  if (state.view === "settings") return settingsView();
  if (!state.connected) {
    return [
      text("连接 Jenkins", { bold: true, size: 12.5 }),
      hint("填写 Jenkins 地址、用户名与密码 / API Token，即可浏览任务、触发构建与查看历史。"),
      btn("打开连接设置", "settings", null, { primary: true })
    ];
  }
  if (!state.job) return [hint("从左侧选择一个任务。")];
  var j = state.job;
  var st = jobStatus(j);
  var children = [];
  var repo = repoForJob(j.url);
  children.push(row([
    text(j.name || "", { bold: true, size: 12.5 }),
    badge(st.text, st.tone),
    badge(jobType(j._class), "muted"),
    repo ? badge("关联 " + repo.name, "accent") : null,
    spacer(),
    j.buildable === false ? badge("不可构建", "muted") : null,
    btn("打开", "openJob", null, { small: true })
  ], { gap: 5 }));
  if (j.description) children.push(text(j.description, { size: 10.5, muted: true }));
  children.push(row([
    j.lastBuild ? text("最近 #" + j.lastBuild.number + " · " + resultLabel(j.lastBuild.result, j.lastBuild.building), { size: 9.5, muted: true, mono: true }) : text("尚无构建", { size: 9.5, muted: true }),
    j.inQueue ? badge("队列中", "accent") : null,
    healthBadge(j),
    spacer(),
    btn("刷新", "loadJob", null, { small: true, disabled: state.loading }),
    btn("构建", "build", null, { primary: true, small: true, disabled: state.loading || j.buildable === false }),
    btn("历史", "loadJob", null, { small: true, disabled: state.loading })
  ], { gap: 5 }));
  children.push(associationRow(j));
  var params = extractParams(j);
  if (params.length) children.push(paramsForm(params));
  if (state.build) children.push(buildDetail());
  children.push(divider());
  children.push(row([text("构建历史", { bold: true, size: 10.5 }), spacer(), badge(state.builds.length + " 次", "muted")], { gap: 4 }));
  var hist = buildHistory();
  if (hist.length) children.push(list(hist, { maxHeight: 185, scroll: true }));
  else children.push(hint("这个任务还没有构建历史。"));
  return children;
}
function healthBadge(j) {
  var reports = j.healthReport || [];
  if (!reports.length) return null;
  return badge("健康 " + reports[0].score + "%", reports[0].score >= 80 ? "ok" : reports[0].score >= 50 ? "warn" : "err");
}
function associationRow(j) {
  var children = [text("关联仓库", { muted: true, size: 9.5 })];
  if (!state.repos.length) {
    children.push(hint("工作区没有仓库"));
    return row(children, { gap: 4 });
  }
  var current = state.associations[ensureSlash(j.url)] || "";
  for (var i = 0; i < state.repos.length && i < 8; i++) {
    var r = state.repos[i];
    children.push(btn(r.name, "associate", { url: j.url, repoID: r.id }, {
      small: true,
      active: current === r.id
    }));
  }
  if (current) children.push(btn("解除关联", "associate", { url: j.url, repoID: "" }, { small: true }));
  return row(children, { gap: 3 });
}
function paramsForm(params) {
  var children = [text("构建参数", { bold: true, size: 10.5 })];
  for (var i = 0; i < params.length && i < 12; i++) {
    var p = params[i];
    var val = selectedParam(p.name);
    if (p.type === "BooleanParameterDefinition") {
      children.push(row([
        text(p.name, { muted: true, size: 9.5, width: 140 }),
        el("checkbox", {
          key: "param:" + p.name,
          text: String(val) === "true" ? "true" : "false",
          checked: String(val) === "true",
          action: "paramBool",
          payload: { name: p.name }
        })
      ], { gap: 4 }));
    } else {
      if (p.choices && p.choices.length) {
        var choices = [text(p.name, { muted: true, size: 9.5, width: 140 })];
        for (var c = 0; c < p.choices.length && c < 8; c++) {
          choices.push(btn(String(p.choices[c]), "paramChoice", { name: p.name, value: String(p.choices[c]) }, {
            small: true,
            active: String(val) === String(p.choices[c])
          }));
        }
        children.push(row(choices, { gap: 3 }));
      } else {
        children.push(row([
          text(p.name, { muted: true, size: 9.5, width: 140 }),
          input("param:" + p.name + "@" + state.selected, val, p.defaultValue || p.description || p.type, {
            action: "param",
            grow: true,
            password: p.type === "PasswordParameterDefinition"
          })
        ], { gap: 4 }));
      }
    }
  }
  if (params.length > 12) children.push(hint("仅显示前 12 个参数，其余使用默认值。"));
  return col(children, { gap: 4 });
}
function buildDetail() {
  var b = state.build;
  var children = [];
  children.push(row([
    text("构建 #" + b.number, { bold: true, size: 11 }),
    badge(resultLabel(b.result, b.building), buildTone(b.result, b.building)),
    text(durationText(b.duration), { muted: true, size: 9.5 }),
    spacer(),
    btn("刷新构建", "refreshBuild", null, { small: true })
  ], { gap: 5 }));
  var items = [];
  var params = buildParams(b);
  for (var i = 0; i < params.length; i++) items.push({ text: params[i].name, detail: String(params[i].value), mono: true });
  if (items.length) children.push(list(items, { maxHeight: 90, scroll: true }));
  if (state.console) children.push(code(state.console, { maxHeight: 240, scroll: true, follow: !!b.building }));
  else children.push(hint("控制台输出会在构建开始后显示。"));
  return col(children, { gap: 5 });
}
function buildParams(b) {
  var out = [];
  var actions = (b && b.actions) || [];
  for (var i = 0; i < actions.length; i++) {
    var params = actions[i].parameters || [];
    for (var j = 0; j < params.length; j++) {
      var p = params[j];
      if (!p || !p.name) continue;
      var v = p.value;
      if (v && typeof v === "object") v = v.length !== undefined ? v.join(", ") : JSON.stringify(v);
      out.push({ name: p.name, value: v });
    }
  }
  return out;
}
function settingsView() {
  var k = state.settingsKey;
  return [
    text("Jenkins 连接", { bold: true, size: 12 }),
    text("Jenkins 地址", { muted: true, size: 9.5 }),
    input("baseUrl", state.settings.baseUrl, "http://jenkins.example.com:8080/", { action: "setting" }),
    row([
      col([
        text("用户名", { muted: true, size: 9.5 }),
        input("username", state.settings.username, "user", { action: "setting", grow: true })
      ], { grow: true, gap: 3 }),
      col([
        text("密码 / API Token", { muted: true, size: 9.5 }),
        input("password", state.settings.password, "API Token", { action: "setting", password: true, grow: true })
      ], { grow: true, gap: 3 })
    ], { gap: 8 }),
    el("checkbox", {
      key: "insecure:" + k,
      text: "跳过 TLS 校验（自签名 Jenkins）",
      checked: state.settings.insecure,
      action: "settingBool",
      payload: { key: "insecure" }
    }),
    row([
      btn("保存并连接", "saveConnect", null, { primary: true }),
      btn("仅保存", "saveSettings", null, { small: true }),
      spacer(),
      btn("清除会话", "resetSession", null, { small: true })
    ], { gap: 4 }),
    hint("会话 Cookie、CSRF Crumb 由插件自动处理；认证失败时请确认账号有 Job/Read、Build 权限。"),
    hint("已支持自由风格、Pipeline、Maven、矩阵、多分支与文件夹任务；构建参数按 Jenkins API 自动生成。"),
    hint("工作区：" + (nemorous.workspace().path || "未打开") + " · 仓库：" + state.repos.length)
  ];
}

// ---------- actions ----------
function handleAction(action, payload, panelID) {
  var p = payload || {};
  switch (action) {
    case "open":
      syncRepos();
      if (state.settings.username && state.settings.password) connect();
      else state.view = "settings";
      break;
    case "openJobs":
      state.view = "jobs";
      if (!state.connected) connect();
      break;
    case "settings":
      state.view = "settings";
      break;
    case "view":
      state.view = p.value || "jobs";
      if (state.view === "jobs" && !state.connected) connect();
      break;
    case "refresh":
      if (state.view === "settings") break;
      if (state.connected) connect();
      break;
    case "filter":
      state.filter = String(p.value || "");
      break;
    case "toggleAssociated":
      state.onlyAssociated = !state.onlyAssociated;
      break;
    case "selectJob":
      loadJob(p.url || "");
      return;
    case "loadJob":
      if (state.job) loadJob(state.job.url);
      return;
    case "openJob":
      if (state.job && state.job.url) nemorous.openURL(state.job.url);
      return;
    case "build":
      triggerBuild();
      return;
    case "param":
      var pn = p.name || String(p.key || "").replace(/^param:/, "");
      var at = pn.indexOf("@");
      if (at >= 0) pn = pn.substring(0, at);
      setParam(pn, p.value);
      break;
    case "paramChoice":
      setParam(p.name, p.value);
      break;
    case "paramBool":
      setParam(p.name, String(p.selected) === "true" || String(p.checked) === "true" ? "true" : "false");
      break;
    case "selectBuild":
      loadBuild(p.url || "");
      return;
    case "refreshBuild":
      if (state.buildURL) loadBuild(state.buildURL);
      return;
    case "associate":
      if (!p.url) break;
      var key = ensureSlash(p.url);
      if (p.repoID) state.associations[key] = p.repoID;
      else delete state.associations[key];
      saveAssociations();
      if (state.job) loadJob(state.job.url);
      return;
    case "setting":
      if (!p.key) break;
      state.settings[p.key] = String(p.value == null ? "" : p.value);
      break;
    case "settingBool":
      if (p.key) state.settings[p.key] = !!p.checked;
      break;
    case "saveSettings":
      saveSettings();
      state.status = "设置已保存";
      state.statusTone = "ok";
      break;
    case "saveConnect":
      state.settings.baseUrl = (state.settings.baseUrl || "").trim();
      if (!/^https?:\/\//.test(state.settings.baseUrl)) {
        state.error = "地址必须以 http:// 或 https:// 开头";
        state.status = "地址无效";
        state.statusTone = "err";
        break;
      }
      saveSettings();
      connect();
      return;
    case "resetSession":
      stopPolling();
      state.crumb = { field: "", value: "" };
      state.cookies = {};
      state.connected = false;
      state.job = null;
      state.build = null;
      state.queue = null;
      state.status = "会话已清除";
      state.statusTone = "muted";
      break;
    case "input":
      if (p.key) {
        // Input keys not handled above are ignored; settings uses setting action.
      }
      return;
    default:
      return;
  }
  nemorous.invalidate();
}

function render(panelID) {
  var children = [header(), tabs()];
  if (state.loading) children.push({ type: "spinner", text: state.status || "正在读取 Jenkins…" });
  else if (state.status) children.push(text(state.status, { size: 9.5, muted: true, tone: state.statusTone === "muted" ? "" : state.statusTone }));
  if (state.error) children.push(text(state.error, { tone: "err", size: 9.5 }));
  if (state.view === "settings") {
    var settings = settingsView();
    for (var i = 0; i < settings.length; i++) children.push(settings[i]);
  } else if (!state.connected) {
    children.push(text("连接 Jenkins", { bold: true, size: 12.5 }));
    children.push(hint("打开设置填写地址、用户名与密码 / API Token。"));
    children.push(btn("连接设置", "settings", null, { primary: true }));
  } else {
    jobsView(children);
  }
  return col(children, { grow: true, padding: 8, gap: 6 });
}
