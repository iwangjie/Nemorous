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
  deletePodConfirm: "",
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
function clean(children) {
  var out = [];
  for (var i = 0; i < (children || []).length; i++) if (children[i]) out.push(children[i]);
  return out;
}
function spacer() { return { type: "spacer" }; }
function divider() { return { type: "divider" }; }
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
    state.status = "已加载 " + state.ns + " · " + state.pods.length + " Pods · " + state.deployments.length + " Deployments";
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
  }), { onError: fail });
  get("/apis/apps/v1" + nsPath + "/deployments?limit=500", guard(function (data) {
    state.deployments = (data.items || []).map(normalizeWorkload);
  }), { onError: fail });
  get("/apis/apps/v1" + nsPath + "/statefulsets?limit=500", guard(function (data) {
    state.statefulsets = (data.items || []).map(normalizeWorkload);
  }), { onError: fail });
  get("/apis/apps/v1" + nsPath + "/daemonsets?limit=500", guard(function (data) {
    state.daemonsets = (data.items || []).map(normalizeWorkload);
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
    created: meta.creationTimestamp,
    raw: s
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
function scaleWorkload(w, delta) {
  if (!w || (w.kind !== "Deployment" && w.kind !== "StatefulSet")) {
    state.error = "只有 Deployment / StatefulSet 支持扩缩容";
    return;
  }
  var replicas = Math.max(0, (w.raw.spec && w.raw.spec.replicas !== undefined ? w.raw.spec.replicas : w.desired) + delta);
  var path = "/apis/apps/v1/namespaces/" + encPath(w.namespace) + "/" + (w.kind === "Deployment" ? "deployments" : "statefulsets") + "/" + encPath(w.name) + "/scale";
  apiRequest("PATCH", path, JSON.stringify({ spec: { replicas: replicas } }), "application/merge-patch+json", function () {
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
    badge(state.connected ? "已连接" : "未连接", state.connected ? "ok" : "muted"),
    state.version ? badge("K8s " + state.version, "muted") : null,
    state.ns ? badge("ns/" + state.ns, "accent") : null,
    state.nodes.length ? badge(state.nodes.length + " 节点", "muted") : null,
    spacer(),
    btn("刷新", "refresh", null, { small: true, disabled: state.loading }),
    btn("设置", "settings", null, { small: true, active: state.view === "settings" })
  ], { gap: 5 });
}
function tabs() {
  var defs = [["overview", "概览"], ["pods", "Pods"], ["workloads", "工作负载"], ["network", "网络"], ["events", "事件"], ["nodes", "节点"]];
  var children = [];
  for (var i = 0; i < defs.length; i++) children.push(btn(defs[i][1], "tab", { value: defs[i][0] }, { small: true, active: state.tab === defs[i][0] }));
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
      detail: n.phase || "",
      action: "selectNS",
      payload: { value: n.name },
      selected: n.name === state.ns
    });
  }
  return col([
    input("nsFilter", state.nsFilter, "过滤命名空间", { action: "filterNS" }),
    list(items, { grow: true, scroll: true }),
    text(state.namespaces.length + " 个命名空间", { muted: true, size: 9.5 })
  ], { width: 210, heightPercent: 100, gap: 5 });
}
function dashboard(children) {
  var content = [tabs()].concat(contentView());
  children.push(row([
    namespaceSidebar(),
    { type: "divider", vertical: true },
    col(content, { grow: true, heightPercent: 100, gap: 6 })
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
  var children = [];
  children.push(row([
    badge(state.pods.length + " Pods", "muted"),
    badge(running + " Running", "ok"),
    badge(pending + " Pending", pending ? "warn" : "muted"),
    badge(failed + " Failed", failed ? "err" : "muted"),
    badge(restarts + " 重启", restarts ? "warn" : "muted"),
    badge(state.deployments.length + " Deployments", "accent")
  ], { gap: 4 }));
  if (state.pods.length) {
    children.push(text("Pod 状态", { bold: true, size: 10.5 }));
    children.push(el("bar", { text: "Running", detail: running + " / " + state.pods.length, fraction: running / state.pods.length, tone: "ok" }));
    if (pending) children.push(el("bar", { text: "Pending", detail: pending + " / " + state.pods.length, fraction: pending / state.pods.length, tone: "warn" }));
    if (failed) children.push(el("bar", { text: "Failed", detail: failed + " / " + state.pods.length, fraction: failed / state.pods.length, tone: "err" }));
  }
  children.push(divider());
  children.push(text("工作负载就绪度", { bold: true, size: 10.5 }));
  if (!state.deployments.length && !state.statefulsets.length) children.push(hint("当前命名空间没有 Deployment / StatefulSet。"));
  for (var d = 0; d < state.deployments.length && d < 8; d++) {
    var w = state.deployments[d];
    children.push(el("bar", { text: w.name, detail: w.ready + " / " + w.desired + " 就绪", fraction: w.desired ? Math.min(1, w.ready / w.desired) : 0, tone: w.ready >= w.desired && w.desired > 0 ? "ok" : "warn" }));
  }
  for (var s = 0; s < state.statefulsets.length && s < 5; s++) {
    var st = state.statefulsets[s];
    children.push(el("bar", { text: st.name + " (STS)", detail: st.ready + " / " + st.desired + " 就绪", fraction: st.desired ? Math.min(1, st.ready / st.desired) : 0, tone: st.ready >= st.desired && st.desired > 0 ? "ok" : "warn" }));
  }
  children.push(divider());
  children.push(row([text("Pod 动态", { bold: true, size: 10.5 }), spacer(), btn("查看全部", "tab", { value: "pods" }, { small: true })], { gap: 4 }));
  var items = [];
  for (var k = 0; k < state.pods.length && k < 8; k++) items.push(podItem(state.pods[k]));
  if (items.length) children.push(list(items, { maxHeight: 180, scroll: true }));
  else children.push(hint("当前命名空间没有 Pod。"));
  return children;
}
function podItem(p) {
  return {
    text: p.name,
    detail: p.phase + " · " + p.ready + "/" + p.total + " · 重启 " + p.restarts + " · " + age(p.created),
    tone: podTone(p),
    action: "selectPod",
    payload: { name: p.name, namespace: p.namespace },
    selected: state.selectedPod && state.selectedPod.name === p.name,
    mono: true
  };
}
function podsView() {
  var children = [];
  children.push(row([text("Pods", { bold: true, size: 11 }), badge(state.pods.length + " 个", "muted"), spacer(), btn("刷新", "refresh", null, { small: true })]));
  var items = [];
  for (var i = 0; i < state.pods.length; i++) items.push(podItem(state.pods[i]));
  if (items.length) children.push(list(items, { maxHeight: 260, scroll: true }));
  else children.push(hint("当前命名空间没有 Pod。"));
  if (state.selectedPod) children = children.concat(podDetail(state.selectedPod));
  return children;
}
function podDetail(p) {
  var children = [];
  children.push(divider());
  children.push(row([
    text(p.name, { bold: true, size: 11, mono: true }),
    badge(p.phase, podTone(p)),
    text("重启 " + p.restarts, { muted: true, size: 9.5 }),
    spacer(),
    btn(state.deletePodConfirm === p.name ? "确认删除" : "删除 Pod", "deletePod", null, { small: true, primary: state.deletePodConfirm === p.name })
  ], { gap: 4 }));
  children.push(text("节点 " + (p.node || "—") + " · IP " + (p.ip || "—") + " · " + p.containers.length + " 容器", { muted: true, size: 9.5, mono: true }));
  for (var i = 0; i < p.containers.length; i++) {
    var c = p.containers[i];
    children.push(row([
      badge(c.name, c.ready ? "ok" : "warn"),
      text(c.image || "", { size: 9.5, muted: true, mono: true, grow: true }),
      text(c.state + " · 重启 " + c.restarts, { size: 9.5, muted: true })
    ], { gap: 4 }));
  }
  children.push(row([text("容器日志", { bold: true, size: 10 }), spacer(), btn("重新加载", "reloadLogs", null, { small: true })]));
  if (state.podLogs) children.push(code(state.podLogs, { maxHeight: 250, scroll: true, follow: true }));
  else children.push(hint("正在读取日志…"));
  return children;
}
function workloadsView() {
  var children = [];
  var all = [];
  for (var i = 0; i < state.deployments.length; i++) all.push(state.deployments[i]);
  for (i = 0; i < state.statefulsets.length; i++) all.push(state.statefulsets[i]);
  for (i = 0; i < state.daemonsets.length; i++) all.push(state.daemonsets[i]);
  children.push(row([text("工作负载", { bold: true, size: 11 }), badge(all.length + " 个", "muted"), spacer(), btn("刷新", "refresh", null, { small: true })]));
  var items = [];
  for (i = 0; i < all.length; i++) {
    var w = all[i];
    items.push({
      text: w.kind + " / " + w.name,
      detail: w.ready + " / " + w.desired + " 就绪 · " + age(w.created),
      tone: w.ready >= w.desired && w.desired > 0 ? "ok" : "warn",
      action: "selectWorkload",
      payload: { kind: w.kind, name: w.name },
      selected: state.selectedWorkload && state.selectedWorkload.name === w.name && state.selectedWorkload.kind === w.kind,
      mono: true
    });
  }
  if (items.length) children.push(list(items, { maxHeight: 240, scroll: true }));
  else children.push(hint("当前命名空间没有工作负载。"));
  if (state.selectedWorkload) children = children.concat(workloadDetail(state.selectedWorkload));
  if (state.jobs.length || state.cronjobs.length) {
    children.push(divider());
    children.push(text("Job / CronJob", { bold: true, size: 10 }));
    var jitems = [];
    for (i = 0; i < state.jobs.length; i++) jitems.push({ text: "Job / " + state.jobs[i].name, detail: state.jobs[i].kind, mono: true });
    for (i = 0; i < state.cronjobs.length; i++) jitems.push({ text: "CronJob / " + state.cronjobs[i].name, detail: state.cronjobs[i].kind, mono: true });
    children.push(list(jitems, { maxHeight: 110, scroll: true }));
  }
  return children;
}
function workloadDetail(w) {
  if (!w) return hint("选择一个工作负载查看详情。");
  var children = [divider()];
  children.push(row([
    text(w.kind + " / " + w.name, { bold: true, size: 11, mono: true }),
    badge(w.ready + "/" + w.desired, w.ready >= w.desired && w.desired > 0 ? "ok" : "warn"),
    spacer(),
    (w.kind === "Deployment" || w.kind === "StatefulSet") ? btn("−1", "scale", { kind: w.kind, name: w.name, delta: -1 }, { small: true }) : null,
    (w.kind === "Deployment" || w.kind === "StatefulSet") ? btn("+1", "scale", { kind: w.kind, name: w.name, delta: 1 }, { small: true }) : null,
    btn("滚动重启", "restart", null, { small: true })
  ], { gap: 4 }));
  if (w.images.length) children.push(text(w.images.join(" · "), { muted: true, size: 9.5, mono: true }));
  var items = [];
  for (var i = 0; i < w.conditions.length; i++) {
    var c = w.conditions[i];
    items.push({ text: c.type, detail: (c.status || "") + (c.reason ? " · " + c.reason : ""), tone: c.status === "True" ? "ok" : "warn", mono: true });
  }
  if (items.length) children.push(list(items, { maxHeight: 110, scroll: true }));
  return children;
}
function networkView() {
  var children = [];
  children.push(row([text("Services", { bold: true, size: 10.5 }), badge(state.services.length + " 个", "muted")]));
  var items = [];
  for (var i = 0; i < state.services.length; i++) {
    var s = state.services[i];
    items.push({ text: s.name, detail: s.type + " · " + s.clusterIP + (s.ports ? " · " + s.ports : ""), mono: true });
  }
  if (items.length) children.push(list(items, { maxHeight: 150, scroll: true }));
  else children.push(hint("当前命名空间没有 Service。"));
  children.push(divider());
  children.push(row([text("Ingress", { bold: true, size: 10.5 }), badge(state.ingresses.length + " 个", "muted")]));
  var ings = [];
  for (i = 0; i < state.ingresses.length; i++) {
    ings.push({ text: state.ingresses[i].name, detail: state.ingresses[i].hosts.join(" · ") || "无规则", mono: true });
  }
  if (ings.length) children.push(list(ings, { maxHeight: 150, scroll: true }));
  else children.push(hint("当前命名空间没有 Ingress。"));
  return children;
}
function eventsView() {
  var children = [];
  children.push(row([text("事件", { bold: true, size: 11 }), badge(state.events.length + " 条", "muted"), spacer(), btn("刷新", "refresh", null, { small: true })]));
  var items = [];
  for (var i = 0; i < state.events.length && i < 60; i++) {
    var e = state.events[i];
    items.push({
      text: e.reason + " · " + e.object,
      detail: e.message + " · " + e.count + "× · " + age(e.last),
      tone: e.type === "Warning" ? "warn" : "muted",
      mono: false
    });
  }
  if (items.length) children.push(list(items, { grow: true, scroll: true }));
  else children.push(hint("当前命名空间没有事件。"));
  return children;
}
function nodesView() {
  var children = [];
  children.push(row([text("节点", { bold: true, size: 11 }), badge(state.nodes.length + " 个", "muted"), spacer(), btn("刷新", "refresh", null, { small: true })]));
  var items = [];
  for (var i = 0; i < state.nodes.length; i++) {
    var n = state.nodes[i];
    items.push({
      text: n.name,
      detail: n.ready + " · " + n.roles + " · " + n.cpu + " CPU · " + n.memory + " · " + n.version,
      tone: n.ready === "Ready" ? "ok" : "err",
      action: "selectNode",
      payload: { name: n.name },
      selected: state.selectedNode && state.selectedNode.name === n.name,
      mono: true
    });
  }
  if (items.length) children.push(list(items, { maxHeight: 260, scroll: true }));
  else children.push(hint("无法读取节点，或当前 Token 没有节点读取权限。"));
  if (state.selectedNode) {
    children.push(divider());
    children.push(row([text(state.selectedNode.name, { bold: true, size: 11, mono: true }), badge(state.selectedNode.ready, state.selectedNode.ready === "Ready" ? "ok" : "err")]));
    children.push(text("IP " + state.selectedNode.ip + " · " + state.selectedNode.cpu + " CPU · " + state.selectedNode.memory + " 内存 · Pod 上限 " + state.selectedNode.pods, { size: 9.5, muted: true, mono: true }));
  }
  return children;
}
function settingsView() {
  var k = state.settingsKey;
  return [
    text("Kubernetes 连接", { bold: true, size: 12 }),
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
    ], { gap: 4 }),
    hint("Token 保存在本地插件存储中，不会写入工作区。建议使用只读 ServiceAccount；删除 Pod / 扩缩容需要对应写权限。"),
    hint("兼容 Kubernetes 1.19+；Ingress 自动尝试 networking.k8s.io/v1 与 extensions/v1beta1。"),
    state.version ? badge("集群 " + state.version, "ok") : null
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
    case "openSettings":
      state.view = "settings";
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
      break;
    case "scale":
      scaleWorkload(findWorkload(p.kind, p.name), Number(p.delta || 0));
      return;
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
