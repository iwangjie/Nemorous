// Kubernetes dashboard plugin. It talks directly to the API server with a
// bearer token, so it works with any conformant cluster from 1.19 onward and
// does not require kubectl on the user's machine.

var state = {
  settings: {
    baseUrl: "https://127.0.0.1:6443",
    token: "",
    namespace: "default",
    insecure: true,
    autoRefresh: true
  },
  settingsKey: 0,
  view: "dashboard",
  tab: "overview",
  connected: false,
  version: "",
  namespaces: [],
  nsFilter: "",
  ns: "",
  podFilter: "",
  workloadFilter: "",
  eventWarningsOnly: false,
  lastUpdated: "",
  pending: 0,
  loading: false,
  status: "",
  statusTone: "",
  error: "",
  pods: [],
  deployments: [],
  statefulsets: [],
  daemonsets: [],
  jobs: [],
  cronjobs: [],
  services: [],
  ingresses: [],
  events: [],
  nodes: [],
  selectedPod: null,
  podLogs: "",
  selectedWorkload: null,
  selectedNode: null,
  selectedService: null,
  selectedIngress: null,
  deletePodConfirm: "",
  scalePending: null,
  pollTimer: 0,
  seq: 0
};

function activate() {
  var s = nemorous.storage.get("settings");
  if (s && typeof s === "object") {
    state.settings.baseUrl = s.baseUrl || state.settings.baseUrl;
    state.settings.token = s.token || "";
    state.settings.namespace = s.namespace || "default";
    state.settings.insecure = s.insecure !== false;
    state.settings.autoRefresh = s.autoRefresh !== false;
  }
}
function onWorkspaceChanged() {}
function saveSettings() {
  nemorous.storage.set("settings", state.settings);
  if (!state.settings.autoRefresh) stopAutoRefresh();
  else if (state.connected) startAutoRefresh();
}

// ---------- UI helpers ----------
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
function card(children, props) {
  var n = props || {};
  n.type = "card";
  n.children = clean(children);
  return n;
}
function clean(children) {
  var out = [];
  for (var i = 0; i < (children || []).length; i++) if (children[i]) out.push(children[i]);
  return out;
}
function spacer() { return { type: "spacer" }; }
function divider() { return { type: "divider" }; }
function metric(label, value, tone, sub) {
  return card([
    text(String(value), { bold: true, size: 18, tone: tone || "" }),
    text(label, { size: 9.5, muted: true }),
    sub ? text(sub, { size: 8.5, muted: true }) : null
  ], { padding: 9, gap: 1, widthPercent: 32 });
}
function sectionTitle(value, extra) {
  return row([text(value, { bold: true, size: 10.5 }), extra || null, spacer()], { gap: 4 });
}
function shortLabel(value, max) {
  var s = String(value || "");
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}
function baseURL() { return String(state.settings.baseUrl || "").replace(/\/+$/, ""); }
function tail(s, max) { s = String(s || ""); return s.length > max ? s.substring(s.length - max) : s; }
function age(ts) {
  if (!ts) return "—";
  var ms = Date.now() - new Date(ts).getTime();
  if (ms < 0) ms = 0;
  var m = Math.floor(ms / 60000);
  if (m < 1) return Math.floor(ms / 1000) + "s";
  if (m < 60) return m + "m";
  var h = Math.floor(m / 60);
  if (h < 24) return h + "h" + (m % 60 ? (m % 60) + "m" : "");
  var d = Math.floor(h / 24);
  return d + "d" + (h % 24 ? (h % 24) + "h" : "");
}
function headerValue(headers, name) {
  if (!headers) return "";
  var low = String(name).toLowerCase();
  for (var k in headers) if (headers.hasOwnProperty(k) && k.toLowerCase() === low) return String(headers[k] || "");
  return "";
}
function apiError(resp) {
  var body = String((resp && resp.body) || "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
  if (body.length > 300) body = body.slice(0, 300);
  return "HTTP " + (resp && resp.status) + (body ? " · " + body : "");
}
function encPath(v) { return encodeURIComponent(String(v || "")); }

// ---------- API ----------
function apiRequest(method, path, body, contentType, onOK, opts) {
  opts = opts || {};
  var headers = {};
  if (state.settings.token) headers.Authorization = "Bearer " + state.settings.token;
  if (contentType) headers["Content-Type"] = contentType;
  var completed = false;
  try {
    nemorous.http.request({
      method: method,
      url: baseURL() + path,
      headers: headers,
      body: body || "",
      timeoutMs: 25000,
      insecure: !!state.settings.insecure,
      onResponse: function (resp) {
        if (completed) return;
        completed = true;
        if (!resp.ok) {
          if (opts.onError) opts.onError(resp);
          else setError("Kubernetes 请求失败：" + apiError(resp));
          return;
        }
        var data = null;
        if (!opts.raw) {
          try { data = JSON.parse(resp.body || "{}"); } catch (e) {
            setError("Kubernetes 返回了无效 JSON");
            return;
          }
        }
        if (onOK) onOK(data, resp);
        nemorous.invalidate();
      },
      onError: function (message) {
        if (completed) return;
        completed = true;
        if (opts.onError) opts.onError({ status: 0, body: message });
        else setError("连接失败：" + String(message));
        nemorous.invalidate();
      }
    });
  } catch (e) {
    setError(String(e));
  }
}
function setError(msg) {
  state.error = msg;
  state.status = "请求失败";
  state.statusTone = "err";
  state.loading = false;
  state.pending = 0;
}
function get(path, onOK, opts) {
  apiRequest("GET", path, "", null, onOK, opts);
}
function beginLoad(count) {
  state.pending += count;
  state.loading = true;
  nemorous.invalidate();
}
function doneLoad() {
  state.pending--;
  if (state.pending <= 0) {
    state.pending = 0;
    state.loading = false;
    syncSelections();
    state.lastUpdated = new Date().toLocaleTimeString();
    state.status = "";
    state.statusTone = "ok";
    nemorous.invalidate();
  }
}
function connect() {
  state.error = "";
  state.status = "正在连接 Kubernetes…";
  state.statusTone = "accent";
  state.loading = true;
  state.connected = false;
  get("/version", function (data) {
    state.version = data.gitVersion || (data.major && data.minor ? "v" + data.major + "." + data.minor : "");
    get("/api/v1/namespaces?limit=500", function (res) {
      var items = (res.items || []).map(function (n) {
        return { name: n.metadata.name, phase: n.status && n.status.phase, created: n.metadata.creationTimestamp };
      });
      state.namespaces = items;
      state.connected = true;
      state.view = "dashboard";
      state.status = "已连接 · " + items.length + " 个命名空间";
      state.statusTone = "ok";
      state.loading = false;
      var wanted = state.settings.namespace;
      var found = false;
      for (var i = 0; i < items.length; i++) if (items[i].name === wanted) found = true;
      state.ns = found ? wanted : (items.length ? items[0].name : "default");
      state.settings.namespace = state.ns;
      loadNamespace(state.ns);
      startAutoRefresh();
    }, { onError: function (resp) { failConnect(resp); } });
  }, { onError: function (resp) { failConnect(resp); } });
}
function failConnect(resp) {
  state.connected = false;
  state.loading = false;
  state.status = "连接失败";
  state.statusTone = "err";
  if (resp.status === 401) state.error = "Token 无效或已过期";
  else if (resp.status === 403) state.error = "Token 没有读取集群的权限";
  else state.error = "无法连接 API Server：" + apiError(resp);
}
function loadNamespace(ns, silent) {
  var requested = ns || state.ns || state.settings.namespace || "default";
  var keepSelection = !!silent && requested === state.ns;
  state.ns = requested;
  state.settings.namespace = state.ns;
  var seq = ++state.seq;
  var nsPath = "/namespaces/" + encPath(state.ns);
  state.error = "";
  state.status = silent ? "正在刷新 " + state.ns + "…" : "正在读取 " + state.ns + "…";
  state.statusTone = "accent";
  if (!keepSelection) {
    state.pods = [];
    state.deployments = [];
    state.statefulsets = [];
    state.daemonsets = [];
    state.jobs = [];
    state.cronjobs = [];
    state.services = [];
    state.ingresses = [];
    state.events = [];
    state.selectedPod = null;
    state.podLogs = "";
    state.selectedWorkload = null;
    state.scalePending = null;
    state.selectedService = null;
    state.selectedIngress = null;
  }
  beginLoad(10);
  function guard(fn) {
    return function (data) {
      if (seq === state.seq) fn(data);
      doneLoad();
    };
  }
  function fail() {
    doneLoad();
  }
  get("/api/v1" + nsPath + "/pods?limit=500", guard(function (data) {
    state.pods = (data.items || []).map(normalizePod);
    if (state.selectedPod) {
      var keep = null;
      for (var pi = 0; pi < state.pods.length; pi++) if (state.pods[pi].name === state.selectedPod.name) keep = state.pods[pi];
      state.selectedPod = keep;
      if (!keep) state.podLogs = "";
    }
  }), { onError: fail });
  get("/apis/apps/v1" + nsPath + "/deployments?limit=500", guard(function (data) {
    state.deployments = (data.items || []).map(function (x) { return normalizeWorkload(x, "Deployment"); });
  }), { onError: fail });
  get("/apis/apps/v1" + nsPath + "/statefulsets?limit=500", guard(function (data) {
    state.statefulsets = (data.items || []).map(function (x) { return normalizeWorkload(x, "StatefulSet"); });
  }), { onError: fail });
  get("/apis/apps/v1" + nsPath + "/daemonsets?limit=500", guard(function (data) {
    state.daemonsets = (data.items || []).map(function (x) { return normalizeWorkload(x, "DaemonSet"); });
  }), { onError: fail });
  get("/api/v1" + nsPath + "/services?limit=500", guard(function (data) {
    state.services = (data.items || []).map(normalizeService);
  }), { onError: fail });
  get("/api/v1" + nsPath + "/events?limit=100", guard(function (data) {
    state.events = (data.items || []).map(normalizeEvent);
    state.events.sort(function (a, b) {
      var at = a.last ? new Date(a.last).getTime() : 0;
      var bt = b.last ? new Date(b.last).getTime() : 0;
      return bt - at;
    });
  }), { onError: fail });
  get("/apis/batch/v1" + nsPath + "/jobs?limit=200", guard(function (data) {
    state.jobs = (data.items || []).map(function (x) { return normalizeWorkload(x, "Job"); });
  }), { onError: fail });
  get("/apis/batch/v1" + nsPath + "/cronjobs?limit=200", guard(function (data) {
    state.cronjobs = (data.items || []).map(function (x) { return normalizeWorkload(x, "CronJob"); });
  }), { onError: fail });
  loadIngress(seq, nsPath, guard, fail);
  loadNodes(seq, guard, fail);
}
function loadIngress(seq, nsPath, guard, fail) {
  get("/apis/networking.k8s.io/v1" + nsPath + "/ingresses?limit=500", guard(function (data) {
    state.ingresses = (data.items || []).map(normalizeIngress);
  }), { onError: function (resp) {
    if (resp.status === 404) {
      get("/apis/extensions/v1beta1" + nsPath + "/ingresses?limit=500", guard(function (data) {
        state.ingresses = (data.items || []).map(normalizeIngress);
      }), { onError: fail });
      return;
    }
    fail();
  } });
}
function loadNodes(seq, guard, fail) {
  get("/api/v1/nodes?limit=500", guard(function (data) {
    state.nodes = (data.items || []).map(normalizeNode);
  }), { onError: fail });
}
function startAutoRefresh() {
  stopAutoRefresh();
  if (!state.settings.autoRefresh) return;
  state.pollTimer = setInterval(function () {
    if (state.settings.autoRefresh && state.connected && state.view !== "settings" && !state.loading && state.tab !== "events") {
      loadNamespace(state.ns, true);
    }
  }, 15000);
}
function stopAutoRefresh() {
  if (state.pollTimer) {
    clearInterval(state.pollTimer);
    state.pollTimer = 0;
  }
}

// ---------- normalization ----------
function normalizePod(p) {
  var meta = p.metadata || {};
  var spec = p.spec || {};
  var status = p.status || {};
  var ready = 0;
  var total = 0;
  var restarts = 0;
  var containers = [];
  var cs = status.containerStatuses || [];
  for (var i = 0; i < cs.length; i++) {
    total++;
    if (cs[i].ready) ready++;
    restarts += cs[i].restartCount || 0;
    containers.push({
      name: cs[i].name,
      image: cs[i].image,
      ready: !!cs[i].ready,
      restarts: cs[i].restartCount || 0,
      state: containerState(cs[i]),
      lastState: cs[i].lastState && cs[i].lastState.terminated ? cs[i].lastState.terminated.reason : ""
    });
  }
  var phase = status.phase || "Unknown";
  var reason = status.reason || (cs.length && cs[0].state && cs[0].state.waiting ? cs[0].state.waiting.reason : "");
  return {
    name: meta.name,
    namespace: meta.namespace,
    phase: phase,
    reason: reason,
    ready: ready,
    total: total,
    restarts: restarts,
    node: spec.nodeName || "",
    ip: status.podIP || "",
    created: meta.creationTimestamp,
    containers: containers,
    conditions: status.conditions || [],
    labels: meta.labels || {},
    raw: p
  };
}
function containerState(cs) {
  if (cs.state && cs.state.running) return "Running";
  if (cs.state && cs.state.waiting) return cs.state.waiting.reason || "Waiting";
  if (cs.state && cs.state.terminated) return cs.state.terminated.reason || "Terminated";
  return "Unknown";
}
function podTone(p) {
  if (p.phase === "Running" && p.ready === p.total) return "ok";
  if (p.phase === "Succeeded") return "muted";
  if (p.phase === "Failed" || /CrashLoop|Error|ImagePull|ErrImage|OOM/.test(p.reason)) return "err";
  return "warn";
}
function normalizeWorkload(w, forcedKind) {
  var meta = w.metadata || {};
  var spec = w.spec || {};
  var status = w.status || {};
  var images = [];
  var tmpl = spec.template && spec.template.spec ? spec.template.spec : null;
  if (tmpl) {
    var cs = tmpl.containers || [];
    for (var i = 0; i < cs.length; i++) images.push(cs[i].image);
  }
  var desired = spec.replicas === undefined ? (forcedKind === "DaemonSet" ? (status.desiredNumberScheduled || 0) : 1) : spec.replicas;
  var ready = status.readyReplicas || 0;
  if (forcedKind === "DaemonSet") ready = status.numberReady || 0;
  if (forcedKind === "Job") desired = status.active || 0;
  return {
    kind: forcedKind || "Deployment",
    name: meta.name,
    namespace: meta.namespace,
    desired: desired,
    ready: ready,
    updated: status.updatedReplicas || 0,
    available: status.availableReplicas || 0,
    created: meta.creationTimestamp,
    images: images,
    conditions: status.conditions || [],
    selector: spec.selector || {},
    raw: w
  };
}
function normalizeService(s) {
  var meta = s.metadata || {};
  var spec = s.spec || {};
  var ports = [];
  var ps = spec.ports || [];
  for (var i = 0; i < ps.length; i++) ports.push((ps[i].port || "") + (ps[i].protocol ? "/" + ps[i].protocol : ""));
  return {
    name: meta.name,
    type: spec.type || "ClusterIP",
    clusterIP: spec.clusterIP || "",
    ports: ports.join(", "),
    selector: spec.selector || {},
    raw: s,
    created: meta.creationTimestamp
  };
}
function normalizeIngress(ing) {
  var meta = ing.metadata || {};
  var spec = ing.spec || {};
  var hosts = [];
  var rules = spec.rules || [];
  for (var i = 0; i < rules.length; i++) {
    var host = rules[i].host || "*";
    var paths = [];
    var ps = rules[i].http && rules[i].http.paths ? rules[i].http.paths : [];
    for (var j = 0; j < ps.length; j++) paths.push((ps[j].path || "/") + " → " + (ps[j].backend && (ps[j].backend.service ? ps[j].backend.service.name + ":" + (ps[j].backend.service.port.number || ps[j].backend.service.port.name) : "backend")));
    hosts.push(host + (paths.length ? "  " + paths.join(" · ") : ""));
  }
  return {
    name: meta.name,
    className: spec.ingressClassName || "",
    hosts: hosts,
    tls: (spec.tls || []).length,
    rules: rules,
    created: meta.creationTimestamp,
    raw: ing
  };
}
function normalizeEvent(e) {
  var involved = e.involvedObject || {};
  return {
    type: e.type || "Normal",
    reason: e.reason || "",
    message: e.message || "",
    object: (involved.kind || "") + "/" + (involved.name || ""),
    count: e.count || 1,
    last: e.lastTimestamp || e.eventTime || e.metadata && e.metadata.creationTimestamp,
    raw: e
  };
}
function normalizeNode(n) {
  var meta = n.metadata || {};
  var status = n.status || {};
  var ready = "Unknown";
  var conds = status.conditions || [];
  for (var i = 0; i < conds.length; i++) if (conds[i].type === "Ready") ready = conds[i].status === "True" ? "Ready" : conds[i].reason || "NotReady";
  var roles = [];
  var labels = meta.labels || {};
  for (var k in labels) if (labels.hasOwnProperty(k) && /^node-role\.kubernetes\.io\//.test(k)) roles.push(k.split("/")[1]);
  var capacity = status.capacity || {};
  var addresses = status.addresses || [];
  var ip = "";
  for (i = 0; i < addresses.length; i++) if (addresses[i].type === "InternalIP") ip = addresses[i].address;
  return {
    name: meta.name,
    ready: ready,
    roles: roles.join(", ") || "worker",
    version: status.nodeInfo ? status.nodeInfo.kubeletVersion : "",
    cpu: capacity.cpu || "",
    memory: capacity.memory || "",
    pods: capacity.pods || "",
    ip: ip,
    addresses: addresses,
    conditions: conds,
    created: meta.creationTimestamp,
    raw: n
  };
}

// ---------- pod logs / actions ----------
function selectPod(p) {
  state.selectedPod = p;
  state.podLogs = "";
  state.deletePodConfirm = "";
  nemorous.invalidate();
  if (!p) return;
  get("/api/v1/namespaces/" + encPath(p.namespace) + "/pods/" + encPath(p.name) + "/log?tailLines=300&timestamps=true", function (data, resp) {
    state.podLogs = tail(resp.body, 120000);
    nemorous.invalidate();
  }, { raw: true, onError: function (resp) {
    state.podLogs = "无法读取日志：" + apiError(resp);
    nemorous.invalidate();
  } });
}
function deletePod() {
  var p = state.selectedPod;
  if (!p) return;
  if (state.deletePodConfirm !== p.name) {
    state.deletePodConfirm = p.name;
    nemorous.invalidate();
    return;
  }
  apiRequest("DELETE", "/api/v1/namespaces/" + encPath(p.namespace) + "/pods/" + encPath(p.name), "", null, function () {
    state.status = "已删除 Pod " + p.name;
    state.statusTone = "ok";
    state.selectedPod = null;
    loadNamespace(p.namespace, true);
  }, { onError: function (resp) { setError("删除 Pod 失败：" + apiError(resp)); } });
}
function scaleWorkload(w, replicas) {
  if (!w || (w.kind !== "Deployment" && w.kind !== "StatefulSet")) {
    state.error = "只有 Deployment / StatefulSet 支持扩缩容";
    return;
  }
  var path = "/apis/apps/v1/namespaces/" + encPath(w.namespace) + "/" + (w.kind === "Deployment" ? "deployments" : "statefulsets") + "/" + encPath(w.name) + "/scale";
  state.status = "正在调整 " + w.name + "…";
  state.statusTone = "accent";
  apiRequest("PATCH", path, JSON.stringify({ spec: { replicas: replicas } }), "application/merge-patch+json", function () {
    state.scalePending = null;
    state.status = w.name + " 已调整为 " + replicas + " 副本";
    state.statusTone = "ok";
    loadNamespace(w.namespace, true);
  }, { onError: function (resp) { setError("扩缩容失败：" + apiError(resp)); } });
}
function restartWorkload(w) {
  if (!w || (w.kind !== "Deployment" && w.kind !== "StatefulSet" && w.kind !== "DaemonSet")) return;
  var path = "/apis/apps/v1/namespaces/" + encPath(w.namespace) + "/" + (w.kind === "Deployment" ? "deployments" : w.kind === "StatefulSet" ? "statefulsets" : "daemonsets") + "/" + encPath(w.name);
  var body = {
    spec: {
      template: {
        metadata: {
          annotations: { "kubectl.kubernetes.io/restartedAt": new Date().toISOString() }
        }
      }
    }
  };
  apiRequest("PATCH", path, JSON.stringify(body), "application/merge-patch+json", function () {
    state.status = w.name + " 已触发滚动重启";
    state.statusTone = "ok";
    loadNamespace(w.namespace, true);
  }, { onError: function (resp) { setError("滚动重启失败：" + apiError(resp)); } });
}

// ---------- UI ----------
function header() {
  return row([
    text("Kubernetes", { bold: true, size: 13 }),
    badge(state.connected ? "在线" : "未连接", state.connected ? "ok" : "muted"),
    state.version ? badge(state.version, "muted") : null,
    state.ns ? badge("ns/" + state.ns, "accent") : null,
    state.lastUpdated ? text("更新 " + state.lastUpdated, { muted: true, size: 9 }) : null,
    spacer(),
    btn("刷新", "refresh", null, { small: true, disabled: state.loading }),
    btn("设置", "settings", null, { small: true, active: state.view === "settings" })
  ], { gap: 5 });
}
function tabs() {
  var defs = [
    ["overview", "概览", ""],
    ["pods", "Pods", state.pods.length],
    ["workloads", "工作负载", state.deployments.length + state.statefulsets.length + state.daemonsets.length],
    ["network", "网络", state.services.length + state.ingresses.length],
    ["events", "事件", state.events.length],
    ["nodes", "节点", state.nodes.length]
  ];
  var children = [];
  for (var i = 0; i < defs.length; i++) {
    var label = defs[i][1] + (defs[i][2] !== "" ? " " + defs[i][2] : "");
    children.push(btn(label, "tab", { value: defs[i][0] }, { small: true, active: state.tab === defs[i][0] }));
  }
  return row(children, { gap: 2 });
}
function namespaceSidebar() {
  var items = [];
  var q = String(state.nsFilter || "").toLowerCase();
  for (var i = 0; i < state.namespaces.length; i++) {
    var n = state.namespaces[i];
    if (q && n.name.toLowerCase().indexOf(q) < 0) continue;
    items.push({
      text: n.name,
      detail: n.name === state.ns ? "当前" : (n.phase || "Active"),
      action: "selectNS",
      payload: { value: n.name },
      selected: n.name === state.ns
    });
  }
  return col([
    text("命名空间", { bold: true, size: 10 }),
    input("nsFilter", state.nsFilter, "过滤命名空间", { action: "filterNS" }),
    list(items, { grow: true, scroll: true }),
    text(state.namespaces.length + " 个命名空间", { muted: true, size: 9 })
  ], { width: 190, heightPercent: 100, gap: 5 });
}
function dashboard(children) {
  var body = col(contentView(), { grow: true, gap: 6, scroll: true });
  children.push(row([
    namespaceSidebar(),
    { type: "divider", vertical: true },
    col([tabs(), body], { grow: true, heightPercent: 100, gap: 6 })
  ], { grow: true, gap: 6 }));
}
function contentView() {
  if (state.tab === "pods") return podsView();
  if (state.tab === "workloads") return workloadsView();
  if (state.tab === "network") return networkView();
  if (state.tab === "events") return eventsView();
  if (state.tab === "nodes") return nodesView();
  return overviewView();
}
function overviewView() {
  var running = 0, pending = 0, failed = 0, restarts = 0;
  for (var i = 0; i < state.pods.length; i++) {
    var p = state.pods[i];
    if (p.phase === "Running") running++;
    else if (p.phase === "Pending") pending++;
    else if (p.phase === "Failed") failed++;
    restarts += p.restarts;
  }
  var readyWorkloads = 0;
  var allWorkloads = state.deployments.concat(state.statefulsets).concat(state.daemonsets);
  for (var wi = 0; wi < allWorkloads.length; wi++) if (allWorkloads[wi].desired > 0 && allWorkloads[wi].ready >= allWorkloads[wi].desired) readyWorkloads++;
  var warningEvents = 0;
  for (var ei = 0; ei < state.events.length; ei++) if (state.events[ei].type === "Warning") warningEvents++;
  var children = [];
  children.push(row([
    metric("Running", running, "ok", "共 " + state.pods.length + " Pods"),
    metric("Pending", pending, pending ? "warn" : "muted", "等待调度/启动"),
    metric("Failed", failed, failed ? "err" : "muted", "最近采集")
  ], { gap: 6 }));
  children.push(row([
    metric("重启次数", restarts, restarts > 0 ? "warn" : "ok", "全部容器累计"),
    metric("已就绪", readyWorkloads, readyWorkloads ? "ok" : "warn", "共 " + allWorkloads.length + " 工作负载"),
    metric("Warning", warningEvents, warningEvents ? "warn" : "ok", "最近 " + state.events.length + " 条事件")
  ], { gap: 6 }));
  var readiness = [];
  for (var d = 0; d < state.deployments.length && d < 5; d++) {
    var w = state.deployments[d];
    readiness.push(el("bar", { text: shortLabel(w.name, 24), detail: w.ready + " / " + w.desired, fraction: w.desired ? Math.min(1, w.ready / w.desired) : 0, tone: w.ready >= w.desired && w.desired > 0 ? "ok" : "warn" }));
  }
  for (var st = 0; st < state.statefulsets.length && st < 3; st++) {
    var sw = state.statefulsets[st];
    readiness.push(el("bar", { text: shortLabel(sw.name, 19) + " (STS)", detail: sw.ready + " / " + sw.desired, fraction: sw.desired ? Math.min(1, sw.ready / sw.desired) : 0, tone: sw.ready >= sw.desired && sw.desired > 0 ? "ok" : "warn" }));
  }
  children.push(card([
    sectionTitle("工作负载就绪度", btn("查看全部", "tab", { value: "workloads" }, { small: true })),
    readiness.length ? col(readiness, { gap: 3 }) : hint("当前命名空间没有工作负载。")
  ], { padding: 10, gap: 4 }));
  var podItems = [];
  for (var k = 0; k < state.pods.length && k < 6; k++) podItems.push(podItem(state.pods[k]));
  children.push(card([
    sectionTitle("最近 Pod", btn("查看全部", "tab", { value: "pods" }, { small: true })),
    podItems.length ? list(podItems, { maxHeight: 220, scroll: true }) : hint("当前命名空间没有 Pod。")
  ], { padding: 10, gap: 3 }));
  var warnings = [];
  for (var e = 0; e < state.events.length && warnings.length < 3; e++) if (state.events[e].type === "Warning") warnings.push(state.events[e]);
  if (warnings.length) {
    var wi = [];
    for (var x = 0; x < warnings.length; x++) wi.push({ text: warnings[x].reason + " · " + warnings[x].object, detail: warnings[x].message, tone: "warn" });
    children.push(card([
      sectionTitle("Warning 事件", btn("查看全部", "tab", { value: "events" }, { small: true })),
      list(wi, { maxHeight: 120, scroll: true })
    ], { padding: 10, gap: 3 }));
  }
  return children;
}
function filteredPods() {
  var q = String(state.podFilter || "").toLowerCase();
  var out = [];
  for (var i = 0; i < state.pods.length; i++) {
    var p = state.pods[i];
    if (q && (p.name + " " + p.phase + " " + p.node + " " + p.reason).toLowerCase().indexOf(q) < 0) continue;
    out.push(p);
  }
  return out;
}
function podsView() {
  var pods = filteredPods();
  var children = [];
  children.push(card([
    row([text("Pods", { bold: true, size: 10.5 }), badge(state.pods.length + " 个", "muted"), spacer(), input("podFilter", state.podFilter, "过滤名称 / 状态 / 节点", { action: "filterPods", width: 220, grow: false })], { gap: 4 }),
    pods.length ? list(pods.map(podItem), { maxHeight: 260, scroll: true }) : hint("没有匹配的 Pod。")
  ], { padding: 10, gap: 4 }));
  if (state.selectedPod) children = children.concat(podDetail(state.selectedPod));
  return children;
}
function podItem(p) {
  return {
    text: p.name,
    detail: p.phase + " · " + p.ready + "/" + p.total + " · 重启 " + p.restarts + " · " + (p.node || "未调度") + " · " + age(p.created),
    tone: podTone(p),
    action: "selectPod",
    payload: { name: p.name, namespace: p.namespace },
    selected: state.selectedPod && state.selectedPod.name === p.name,
    mono: true
  };
}
function podDetail(p) {
  var children = [];
  children.push(card([
    row([
      text(p.name, { bold: true, size: 11.5, mono: true }),
      badge(p.phase, podTone(p)),
      badge(p.ready + "/" + p.total, p.ready === p.total ? "ok" : "warn"),
      text("重启 " + p.restarts, { muted: true, size: 9.5 }),
      spacer(),
      btn(state.deletePodConfirm === p.name ? "确认删除" : "删除 Pod", "deletePod", null, { small: true, primary: state.deletePodConfirm === p.name })
    ], { gap: 4 }),
    row([
      text("Node " + (p.node || "—"), { size: 9, muted: true, mono: true }),
      text("IP " + (p.ip || "—"), { size: 9, muted: true, mono: true }),
      text(age(p.created) + " 前创建", { size: 9, muted: true })
    ], { gap: 8 })
  ], { padding: 10, gap: 5 }));
  var containers = [];
  for (var i = 0; i < p.containers.length; i++) {
    var c = p.containers[i];
    containers.push(row([
      badge(c.ready ? "Ready" : c.state, c.ready ? "ok" : "warn"),
      text(c.name, { size: 9.5, bold: true, mono: true, width: 110 }),
      text(c.image || "", { size: 9, muted: true, mono: true, grow: true }),
      text("重启 " + c.restarts, { size: 9, muted: true })
    ], { gap: 4 }));
  }
  children.push(card([
    sectionTitle("容器", badge(p.containers.length + " 个", "muted")),
    containers.length ? col(containers, { gap: 3 }) : hint("没有容器状态。")
  ], { padding: 10, gap: 4 }));
  children.push(card([
    row([text("容器日志", { bold: true, size: 10 }), spacer(), btn("刷新", "reloadLogs", null, { small: true })], { gap: 4 }),
    state.podLogs ? code(state.podLogs, { maxHeight: 320, scroll: true, follow: true }) : hint("正在读取日志…")
  ], { padding: 10, gap: 4 }));
  return children;
}
function filteredWorkloads() {
  var all = [];
  for (var i = 0; i < state.deployments.length; i++) all.push(state.deployments[i]);
  for (i = 0; i < state.statefulsets.length; i++) all.push(state.statefulsets[i]);
  for (i = 0; i < state.daemonsets.length; i++) all.push(state.daemonsets[i]);
  var q = String(state.workloadFilter || "").toLowerCase();
  if (!q) return all;
  var out = [];
  for (i = 0; i < all.length; i++) if ((all[i].kind + " " + all[i].name).toLowerCase().indexOf(q) >= 0) out.push(all[i]);
  return out;
}
function workloadItem(w) {
  return {
    text: w.kind + " / " + w.name,
    detail: w.ready + " / " + w.desired + " 就绪 · " + age(w.created),
    tone: w.ready >= w.desired && w.desired > 0 ? "ok" : "warn",
    action: "selectWorkload",
    payload: { kind: w.kind, name: w.name },
    selected: state.selectedWorkload && state.selectedWorkload.name === w.name && state.selectedWorkload.kind === w.kind,
    mono: true
  };
}
function workloadsView() {
  var all = filteredWorkloads();
  var children = [];
  children.push(card([
    row([text("工作负载", { bold: true, size: 10.5 }), badge(all.length + " 个", "muted"), spacer(), input("workloadFilter", state.workloadFilter, "过滤名称 / 类型", { action: "filterWorkloads", width: 200 })], { gap: 4 }),
    all.length ? list(all.map(workloadItem), { maxHeight: 250, scroll: true }) : hint("没有匹配的工作负载。")
  ], { padding: 10, gap: 4 }));
  if (state.selectedWorkload) children = children.concat(workloadDetail(state.selectedWorkload));
  if (state.jobs.length || state.cronjobs.length) {
    var jitems = [];
    for (var i = 0; i < state.jobs.length; i++) jitems.push({ text: "Job / " + state.jobs[i].name, detail: "Job", mono: true });
    for (i = 0; i < state.cronjobs.length; i++) jitems.push({ text: "CronJob / " + state.cronjobs[i].name, detail: "CronJob", mono: true });
    children.push(card([sectionTitle("Job / CronJob", badge(jitems.length + " 个", "muted")), list(jitems, { maxHeight: 150, scroll: true })], { padding: 10, gap: 4 }));
  }
  return children;
}
function workloadDetail(w) {
  if (!w) return [];
  var children = [];
  children.push(card([
    row([
      text(w.kind + " / " + w.name, { bold: true, size: 11.5, mono: true }),
      badge(w.ready + "/" + w.desired, w.ready >= w.desired && w.desired > 0 ? "ok" : "warn"),
      spacer(),
      (w.kind === "Deployment" || w.kind === "StatefulSet") ? btn(scaleLabel(w, -1), "scale", { kind: w.kind, name: w.name, delta: -1 }, { small: true, active: scalePendingFor(w, -1) }) : null,
      (w.kind === "Deployment" || w.kind === "StatefulSet") ? btn(scaleLabel(w, 1), "scale", { kind: w.kind, name: w.name, delta: 1 }, { small: true, active: scalePendingFor(w, 1) }) : null,
      btn("滚动重启", "restart", null, { small: true })
    ], { gap: 4 }),
    w.images.length ? text(w.images.join(" · "), { muted: true, size: 9, mono: true }) : null,
    state.scalePending && state.scalePending.kind === w.kind && state.scalePending.name === w.name ?
      row([text("确认调整为 " + state.scalePending.target + " 副本？", { size: 9.5, tone: "warn" }), spacer(), btn("取消", "cancelScale", null, { small: true })], { gap: 4 }) : null
  ], { padding: 10, gap: 5 }));
  var items = [];
  for (var i = 0; i < w.conditions.length; i++) {
    var c = w.conditions[i];
    items.push({ text: c.type, detail: (c.status || "") + (c.reason ? " · " + c.reason : ""), tone: c.status === "True" ? "ok" : "warn", mono: true });
  }
  if (items.length) children.push(card([sectionTitle("Conditions", badge(items.length + " 条", "muted")), list(items, { maxHeight: 140, scroll: true })], { padding: 10, gap: 4 }));
  return children;
}
function scaleTarget(w, delta) {
  var current = w.raw.spec && w.raw.spec.replicas !== undefined ? w.raw.spec.replicas : w.desired;
  return Math.max(0, current + delta);
}
function scalePendingFor(w, delta) {
  return !!state.scalePending && state.scalePending.kind === w.kind &&
    state.scalePending.name === w.name && state.scalePending.target === scaleTarget(w, delta);
}
function scaleLabel(w, delta) {
  var label = (delta < 0 ? "−1" : "+1");
  return scalePendingFor(w, delta) ? label + " 确认" : label;
}
function scaleRequest(w, delta) {
  if (!w || (w.kind !== "Deployment" && w.kind !== "StatefulSet")) {
    state.error = "只有 Deployment / StatefulSet 支持扩缩容";
    return;
  }
  var target = scaleTarget(w, delta);
  if (state.scalePending && state.scalePending.kind === w.kind &&
      state.scalePending.name === w.name && state.scalePending.target === target) {
    state.scalePending = null;
    scaleWorkload(w, target);
    return;
  }
  state.scalePending = { kind: w.kind, name: w.name, target: target };
  state.status = "再次点击确认将 " + w.name + " 调整为 " + target + " 副本";
  state.statusTone = "warn";
  nemorous.invalidate();
}

function serviceItem(service) {
  return {
    text: service.name,
    detail: service.type + " · " + service.clusterIP + (service.ports ? " · " + service.ports : ""),
    action: "selectService",
    payload: { name: service.name },
    selected: state.selectedService && state.selectedService.name === service.name,
    mono: true
  };
}
function networkView() {
  var children = [];
  children.push(card([
    sectionTitle("Services", badge(state.services.length + " 个", "muted")),
    state.services.length ? list(state.services.map(serviceItem), { maxHeight: 190, scroll: true }) : hint("当前命名空间没有 Service。")
  ], { padding: 10, gap: 4 }));
  if (state.selectedService) {
    var svc = state.selectedService;
    var selector = [];
    for (var k in svc.selector) if (svc.selector.hasOwnProperty(k)) selector.push(k + "=" + svc.selector[k]);
    children.push(card([
      sectionTitle("Service / " + svc.name, badge(svc.type, "accent")),
      text("ClusterIP " + (svc.clusterIP || "—") + " · " + svc.ports, { size: 9.5, muted: true, mono: true }),
      text("Selector " + (selector.join(", ") || "无"), { size: 9, muted: true, mono: true })
    ], { padding: 10, gap: 4 }));
  }
  var items = [];
  for (var i = 0; i < state.ingresses.length; i++) {
    var ing = state.ingresses[i];
    items.push({
      text: ing.name,
      detail: ing.hosts.join(" · ") || "无规则",
      action: "selectIngress",
      payload: { name: ing.name },
      selected: state.selectedIngress && state.selectedIngress.name === ing.name,
      mono: true
    });
  }
  children.push(card([
    sectionTitle("Ingress", badge(state.ingresses.length + " 个", "muted")),
    items.length ? list(items, { maxHeight: 190, scroll: true }) : hint("当前命名空间没有 Ingress。")
  ], { padding: 10, gap: 4 }));
  if (state.selectedIngress) {
    var sel = state.selectedIngress;
    children.push(card([
      sectionTitle("Ingress / " + sel.name, badge("TLS " + sel.tls, sel.tls ? "ok" : "muted")),
      text(sel.className ? "Class " + sel.className : "默认 class", { size: 9, muted: true, mono: true }),
      text(sel.hosts.join(" · ") || "无规则", { size: 9.5, muted: true, mono: true })
    ], { padding: 10, gap: 4 }));
  }
  return children;
}
function eventsView() {
  var children = [];
  var items = [];
  var warnings = 0;
  for (var i = 0; i < state.events.length && items.length < 80; i++) {
    var e = state.events[i];
    if (e.type === "Warning") warnings++;
    if (state.eventWarningsOnly && e.type !== "Warning") continue;
    items.push({
      text: e.reason + " · " + e.object,
      detail: e.message + " · " + e.count + "× · " + age(e.last),
      tone: e.type === "Warning" ? "warn" : "muted"
    });
  }
  children.push(card([
    row([text("事件", { bold: true, size: 10.5 }), badge(state.events.length + " 条", "muted"), badge(warnings + " Warning", warnings ? "warn" : "muted"), spacer(), btn(state.eventWarningsOnly ? "仅 Warning ✓" : "仅 Warning", "filterWarnings", null, { small: true, active: state.eventWarningsOnly })], { gap: 4 }),
    items.length ? list(items, { maxHeight: 560, scroll: true }) : hint("没有匹配的事件。")
  ], { padding: 10, gap: 4 }));
  return children;
}
function nodeItem(n) {
  return {
    text: n.name,
    detail: n.ready + " · " + n.roles + " · " + n.cpu + " CPU · " + n.memory,
    tone: n.ready === "Ready" ? "ok" : "err",
    action: "selectNode",
    payload: { name: n.name },
    selected: state.selectedNode && state.selectedNode.name === n.name,
    mono: true
  };
}
function nodesView() {
  var children = [];
  children.push(card([
    sectionTitle("集群节点", badge(state.nodes.length + " 个", "muted")),
    state.nodes.length ? list(state.nodes.map(nodeItem), { maxHeight: 260, scroll: true }) : hint("无法读取节点，或当前 Token 没有节点读取权限。")
  ], { padding: 10, gap: 4 }));
  if (state.selectedNode) {
    var n = state.selectedNode;
    children.push(card([
      row([text(n.name, { bold: true, size: 11.5, mono: true }), badge(n.ready, n.ready === "Ready" ? "ok" : "err"), spacer(), text(n.version, { muted: true, size: 9, mono: true })], { gap: 4 }),
      row([text("CPU " + n.cpu, { size: 9.5, muted: true }), text("内存 " + n.memory, { size: 9.5, muted: true }), text("Pod 上限 " + n.pods, { size: 9.5, muted: true }), text(n.ip, { size: 9.5, muted: true, mono: true })], { gap: 8 })
    ], { padding: 10, gap: 5 }));
    var conds = [];
    for (var i = 0; i < n.conditions.length; i++) {
      var c = n.conditions[i];
      conds.push({ text: c.type, detail: (c.status || "") + (c.reason ? " · " + c.reason : ""), tone: c.status === "True" ? "ok" : "warn", mono: true });
    }
    if (conds.length) children.push(card([sectionTitle("Conditions", badge(conds.length + " 条", "muted")), list(conds, { maxHeight: 180, scroll: true })], { padding: 10, gap: 4 }));
  }
  return children;
}
function settingsView() {
  var k = state.settingsKey;
  return [
    row([btn("← 返回仪表盘", "backDashboard", null, { small: true }), spacer()], { gap: 4 }),
    card([
      text("集群连接", { bold: true, size: 11.5 }),
      text("API Server", { muted: true, size: 9.5 }),
      input("baseUrl", state.settings.baseUrl, "https://kubernetes.example.com:6443", { action: "setting" }),
      text("Bearer Token", { muted: true, size: 9.5 }),
      input("token", state.settings.token, "ServiceAccount / 用户 Token", { action: "setting", password: true }),
      row([
        col([
          text("默认命名空间", { muted: true, size: 9.5 }),
          input("namespace", state.settings.namespace, "default", { action: "setting", grow: true })
        ], { grow: true, gap: 3 }),
        col([
          el("checkbox", { key: "insecure:" + k, text: "跳过 TLS 校验", checked: state.settings.insecure, action: "settingBool", payload: { key: "insecure" } }),
          el("checkbox", { key: "autoRefresh:" + k, text: "自动刷新 (15s)", checked: state.settings.autoRefresh, action: "settingBool", payload: { key: "autoRefresh" } })
        ], { grow: true, gap: 3 })
      ], { gap: 8 }),
      row([
        btn("保存并连接", "saveConnect", null, { primary: true }),
        btn("仅保存", "saveSettings", null, { small: true }),
        spacer(),
        btn("停止自动刷新", "stopRefresh", null, { small: true })
      ], { gap: 4 })
    ], { padding: 10, gap: 6 }),
    card([
      text("行为与安全", { bold: true, size: 10.5 }),
      hint("Token 仅保存在本地插件存储，不会写入工作区。建议使用带命名空间读取权限的 ServiceAccount。"),
      hint("删除 Pod、扩缩容与滚动重启需要相应写权限。"),
      hint("兼容 Kubernetes 1.19+；Ingress 自动在 networking.k8s.io/v1 与 extensions/v1beta1 间回退。"),
      state.version ? badge("集群 " + state.version, "ok") : null
    ], { padding: 10, gap: 5 })
  ];
}

// ---------- actions ----------
function findPod(name) {
  for (var i = 0; i < state.pods.length; i++) if (state.pods[i].name === name) return state.pods[i];
  return null;
}
function findWorkload(kind, name) {
  var arr = kind === "Deployment" ? state.deployments : kind === "StatefulSet" ? state.statefulsets : state.daemonsets;
  for (var i = 0; i < arr.length; i++) if (arr[i].name === name && arr[i].kind === kind) return arr[i];
  return null;
}
function findService(name) {
  for (var i = 0; i < state.services.length; i++) if (state.services[i].name === name) return state.services[i];
  return null;
}
function findIngress(name) {
  for (var i = 0; i < state.ingresses.length; i++) if (state.ingresses[i].name === name) return state.ingresses[i];
  return null;
}
function syncSelections() {
  if (state.selectedWorkload) {
    var w = findWorkload(state.selectedWorkload.kind, state.selectedWorkload.name);
    state.selectedWorkload = w;
  }
  if (state.selectedService) state.selectedService = findService(state.selectedService.name);
  if (state.selectedIngress) state.selectedIngress = findIngress(state.selectedIngress.name);
  if (state.selectedNode) {
    var n = null;
    for (var i = 0; i < state.nodes.length; i++) if (state.nodes[i].name === state.selectedNode.name) n = state.nodes[i];
    state.selectedNode = n;
  }
}
function handleAction(action, payload, panelID) {
  var p = payload || {};
  switch (action) {
    case "open":
      if (state.settings.token && state.settings.baseUrl) connect();
      else state.view = "settings";
      break;
    case "openDashboard":
      state.view = "dashboard";
      if (!state.connected) connect();
      break;
    case "settings":
      if (state.view === "settings" && state.connected) state.view = "dashboard";
      else state.view = "settings";
      break;
    case "openSettings":
      state.view = "settings";
      break;
    case "backDashboard":
      state.view = "dashboard";
      break;
    case "settingsView":
      state.view = "settings";
      break;
    case "view":
      state.view = p.value || "dashboard";
      break;
    case "tab":
      state.tab = p.value || "overview";
      break;
    case "refresh":
      if (state.view === "settings") break;
      if (!state.connected) connect();
      else loadNamespace(state.ns, true);
      return;
    case "selectNS":
      state.settings.namespace = p.value || "default";
      saveSettings();
      loadNamespace(state.settings.namespace);
      return;
    case "filterNS":
      state.nsFilter = String(p.value || "");
      break;
    case "filterPods":
      state.podFilter = String(p.value || "");
      break;
    case "filterWorkloads":
      state.workloadFilter = String(p.value || "");
      break;
    case "filterWarnings":
      state.eventWarningsOnly = !state.eventWarningsOnly;
      break;
    case "selectService":
      state.selectedService = findService(p.name);
      break;
    case "selectIngress":
      state.selectedIngress = findIngress(p.name);
      break;
    case "selectPod":
      var pod = findPod(p.name);
      if (pod) selectPod(pod);
      return;
    case "reloadLogs":
      if (state.selectedPod) selectPod(state.selectedPod);
      return;
    case "deletePod":
      deletePod();
      return;
    case "selectWorkload":
      state.selectedWorkload = findWorkload(p.kind, p.name);
      state.scalePending = null;
      break;
    case "scale":
      scaleRequest(findWorkload(p.kind, p.name), Number(p.delta || 0));
      return;
    case "cancelScale":
      state.scalePending = null;
      state.status = "";
      state.statusTone = "";
      break;
    case "restart":
      restartWorkload(state.selectedWorkload);
      return;
    case "selectNode":
      for (var i = 0; i < state.nodes.length; i++) if (state.nodes[i].name === p.name) state.selectedNode = state.nodes[i];
      break;
    case "setting":
      if (p.key) state.settings[p.key] = String(p.value == null ? "" : p.value);
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
      state.settings.baseUrl = String(state.settings.baseUrl || "").trim();
      if (!/^https?:\/\//.test(state.settings.baseUrl)) {
        state.error = "API Server 必须以 http:// 或 https:// 开头";
        state.status = "地址无效";
        state.statusTone = "err";
        break;
      }
      state.settings.token = String(state.settings.token || "").trim();
      saveSettings();
      connect();
      return;
    case "stopRefresh":
      stopAutoRefresh();
      state.settings.autoRefresh = false;
      saveSettings();
      state.status = "已停止自动刷新";
      state.statusTone = "muted";
      break;
    default:
      break;
  }
  nemorous.invalidate();
}

function render(panelID) {
  var children = [header()];
  if (state.loading) children.push({ type: "spinner", text: state.status || "正在读取 Kubernetes…" });
  else if (state.status) children.push(text(state.status, { size: 9.5, muted: true, tone: state.statusTone === "muted" ? "" : state.statusTone }));
  if (state.error) children.push(text(state.error, { tone: "err", size: 9.5 }));
  if (state.view === "settings" || !state.connected) {
    if (!state.connected && state.view !== "settings") {
      children.push(text("连接 Kubernetes", { bold: true, size: 12.5 }));
      children.push(hint("填写 API Server 与 Bearer Token；Token 可以来自 ServiceAccount 或 kubeconfig。"));
      children.push(btn("打开连接设置", "settings", null, { primary: true }));
    } else {
      var settings = settingsView();
      for (var i = 0; i < settings.length; i++) children.push(settings[i]);
    }
  } else {
    dashboard(children);
  }
  return col(children, { grow: true, padding: 8, gap: 6 });
}
