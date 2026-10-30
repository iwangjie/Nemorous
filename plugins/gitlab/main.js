// Nemorous GitLab plugin for private servers.
//
// Settings and calls use the GitLab v4 REST API. The plugin deliberately
// keeps the token in its private host storage; it never writes it to the
// workspace or command line.

var state = {
  settings: { baseUrl: "https://gitlab.com", token: "", project: "", insecure: false },
  settingsKey: 0,
  showSettings: true,
  tab: "mrs",
  data: [],
  selected: null,
  detail: null,
  notes: [],
  loading: false,
  error: "",
  status: "",
  comment: "",
  seq: 0
};

function activate() {
  var s = nemorous.storage.get("settings");
  if (s && typeof s === "object") {
    state.settings = {
      baseUrl: s.baseUrl || "https://gitlab.com",
      token: s.token || "",
      project: s.project || "",
      insecure: !!s.insecure
    };
    state.showSettings = !(state.settings.token && state.settings.project);
  }
}

function onWorkspaceChanged() {
  state.data = [];
  state.selected = null;
  state.detail = null;
  state.notes = [];
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

function saveSettings() {
  state.settings.baseUrl = String(state.settings.baseUrl || "").replace(/\/+$/, "");
  state.settings.project = String(state.settings.project || "").replace(/^\/+|\/+$/g, "");
  if (!/^https?:\/\//.test(state.settings.baseUrl)) {
    state.error = "GitLab 地址必须以 http:// 或 https:// 开头";
    return false;
  }
  if (!state.settings.project) {
    state.error = "请填写项目路径，例如 group/project";
    return false;
  }
  nemorous.storage.set("settings", state.settings);
  state.showSettings = false;
  state.error = "";
  return true;
}

function api(method, path, body, onOK) {
  if (!state.settings.token) {
    state.error = "请先填写 Private Token";
    return;
  }
  var seq = ++state.seq;
  state.loading = true;
  state.error = "";
  state.status = "";
  var url = state.settings.baseUrl + "/api/v4" + path;
  nemorous.http.request({
    method: method,
    url: url,
    headers: { "PRIVATE-TOKEN": state.settings.token },
    body: body ? JSON.stringify(body) : "",
    timeoutMs: 25000,
    insecure: state.settings.insecure,
    onResponse: function (resp) {
      if (seq !== state.seq) return;
      state.loading = false;
      if (!resp.ok) {
        var msg = "HTTP " + resp.status;
        try {
          var payload = JSON.parse(resp.body);
          if (payload && (payload.message || payload.error)) msg = payload.message || payload.error;
        } catch (e) {}
        state.error = msg;
        nemorous.invalidate();
        return;
      }
      var data = null;
      try {
        data = JSON.parse(resp.body);
      } catch (e) {
        state.error = "GitLab 返回了无效 JSON";
        nemorous.invalidate();
        return;
      }
      onOK(data);
      nemorous.invalidate();
    },
    onError: function (message) {
      if (seq !== state.seq) return;
      state.loading = false;
      state.error = "连接失败：" + message;
      nemorous.invalidate();
    }
  });
}

function projectPath() {
  return encodeURIComponent(state.settings.project);
}

function loadCurrent() {
  state.selected = null;
  state.detail = null;
  state.notes = [];
  if (state.tab === "mrs") {
    api("GET", "/projects/" + projectPath() + "/merge_requests?state=opened&per_page=30&order_by=updated_at", null, function (data) {
      state.data = data || [];
    });
  } else if (state.tab === "pipelines") {
    api("GET", "/projects/" + projectPath() + "/pipelines?per_page=30", null, function (data) {
      state.data = data || [];
    });
  } else {
    api("GET", "/projects/" + projectPath() + "/issues?state=opened&per_page=30&order_by=updated_at", null, function (data) {
      state.data = data || [];
    });
  }
}

function loadNotes(iid) {
  api("GET", "/projects/" + projectPath() + "/merge_requests/" + iid + "/notes?sort=desc&per_page=30", null, function (data) {
    state.notes = data || [];
  });
}

function selectItem(item) {
  state.selected = item;
  state.detail = item;
  state.notes = [];
  if (state.tab === "mrs") loadNotes(item.iid);
}

function approve() {
  if (!state.detail) return;
  api("POST", "/projects/" + projectPath() + "/merge_requests/" + state.detail.iid + "/approve", {}, function () {
    state.status = "已批准 !" + state.detail.iid;
    state.detail.approved = true;
  });
}

function comment() {
  if (!state.detail || !String(state.comment || "").trim()) return;
  api("POST", "/projects/" + projectPath() + "/merge_requests/" + state.detail.iid + "/notes", { body: state.comment }, function () {
    state.comment = "";
    loadNotes(state.detail.iid);
    state.status = "评论已发送";
  });
}

function detectProject() {
  var workspace = nemorous.workspace();
  var repos = workspace.repos || [];
  for (var i = 0; i < repos.length; i++) {
    var configPath = repos[i].path + "/.git/config";
    var content = "";
    try {
      content = nemorous.fs.read(configPath);
    } catch (e) {
      continue;
    }
    var match = content.match(/url\s*=\s*([^\s]+)/);
    if (!match) continue;
    var remote = match[1];
    var host = "";
    var path = "";
    var m = remote.match(/^git@([^:]+):(.+?)(?:\.git)?$/);
    if (m) {
      host = m[1];
      path = m[2];
    } else {
      m = remote.match(/^https?:\/\/([^/]+)\/(.+?)(?:\.git)?$/);
      if (m) {
        host = m[1];
        path = m[2];
      }
    }
    if (host && path) {
      state.settings.baseUrl = "https://" + host;
      state.settings.project = path;
      state.settingsKey++;
      state.status = "已识别 " + path;
      return;
    }
  }
  state.error = "没有从仓库 remote 识别到 GitLab 项目";
}

function handleAction(action, payload) {
  var p = payload || {};
  switch (action) {
    case "open":
      if (!state.settings.token || !state.settings.project) {
        if (!state.settings.project) {
          detectProject();
          if (!state.settings.project) state.error = "";
        }
        state.showSettings = true;
      } else {
        loadCurrent();
      }
      break;
    case "refresh":
      if (!state.showSettings) loadCurrent();
      break;
    case "settings":
      state.showSettings = !state.showSettings;
      state.settingsKey++;
      break;
    case "setting":
      if (p.key) {
        var key = String(p.key);
        if (key.indexOf("gl-url:") === 0) state.settings.baseUrl = String(p.value || "");
        else if (key.indexOf("gl-token:") === 0) state.settings.token = String(p.value || "");
        else if (key.indexOf("gl-project:") === 0) state.settings.project = String(p.value || "");
        else if (key.indexOf("gl-insecure:") === 0) state.settings.insecure = !!p.checked;
      }
      break;
    case "save-settings":
      if (saveSettings()) loadCurrent();
      break;
    case "detect":
      detectProject();
      break;
    case "tab":
      state.tab = p.tab || "mrs";
      state.showSettings = false;
      loadCurrent();
      break;
    case "select":
      selectItem(p.item);
      break;
    case "open-detail":
      if (state.detail && state.detail.web_url) nemorous.openURL(state.detail.web_url);
      break;
    case "approve":
      approve();
      break;
    case "comment":
      comment();
      break;
    case "comment-input":
      state.comment = String(p.value || "");
      break;
  }
  nemorous.invalidate();
}

function settingsView() {
  var k = state.settingsKey;
  return el("column", { gap: 6, children: [
    txt("连接公司 GitLab", { size: 11, bold: true }),
    el("row", { gap: 6, children: [
      txt("地址", { size: 10, muted: true, width: 52 }),
      el("input", { key: "gl-url:" + k, value: state.settings.baseUrl, placeholder: "https://gitlab.example.com", action: "setting", grow: true })
    ] }),
    el("row", { gap: 6, children: [
      txt("Token", { size: 10, muted: true, width: 52 }),
      el("input", { key: "gl-token:" + k, value: state.settings.token, placeholder: "Private Token", action: "setting", grow: true, password: true })
    ] }),
    el("row", { gap: 6, children: [
      txt("项目", { size: 10, muted: true, width: 52 }),
      el("input", { key: "gl-project:" + k, value: state.settings.project, placeholder: "group/project", action: "setting", grow: true })
    ] }),
    el("row", { gap: 6, children: [
      el("checkbox", { key: "gl-insecure:" + k, text: "跳过 TLS 校验", checked: state.settings.insecure, action: "setting" }),
      el("spacer"),
      btn("从仓库识别", "detect", null, false),
      btn("保存", "save-settings", null, true)
    ] }),
    txt("Token 保存在本地插件存储中，不会写入工作区。", { size: 9, muted: true })
  ] });
}

function tabs() {
  return el("row", { gap: 4, children: [
    btn("MR", "tab", { tab: "mrs" }, state.tab === "mrs"),
    btn("流水线", "tab", { tab: "pipelines" }, state.tab === "pipelines"),
    btn("Issue", "tab", { tab: "issues" }, state.tab === "issues"),
    el("spacer"),
    btn("刷新", "refresh", null, false),
    btn("设置", "settings", null, false, false)
  ] });
}

function itemList() {
  var items = [];
  for (var i = 0; i < state.data.length; i++) {
    var item = state.data[i];
    var text = "";
    var detail = "";
    if (state.tab === "mrs") {
      text = "!" + item.iid + " " + (item.title || "");
      detail = (item.source_branch || "?") + " → " + (item.target_branch || "?");
    } else if (state.tab === "pipelines") {
      text = "#" + item.id + " " + (item.status || "");
      detail = (item.ref || "") + " · " + (item.sha || "").slice(0, 8);
    } else {
      text = "#" + item.iid + " " + (item.title || "");
      detail = (item.labels || []).join(", ");
    }
    items.push({
      text: text,
      detail: detail,
      action: "select",
      payload: { item: item },
      selected: state.selected && state.selected.id === item.id
    });
  }
  return el("list", { items: items, scroll: true, maxHeight: 210 });
}

function detailView() {
  var d = state.detail;
  if (!d) return txt("选择一个条目查看详情", { muted: true, size: 10.5 });
  var children = [
    el("row", { gap: 5, children: [
      txt(state.tab === "mrs" ? "!" + d.iid : state.tab === "pipelines" ? "#" + d.id : "#" + d.iid, { size: 10, mono: true, muted: true }),
      txt(d.title || d.status || "", { size: 11.5, bold: true, grow: true }),
      btn("打开", "open-detail", null, false)
    ] }),
    el("row", { gap: 5, children: [
      el("badge", { text: d.state || d.status || "", tone: d.state === "opened" || d.status === "success" ? "ok" : "muted" }),
      txt(d.author ? "@" + (d.author.username || d.author.name) : "", { size: 9.5, muted: true })
    ] })
  ];
  if (d.description) children.push(txt(d.description, { size: 10.5, muted: false }));
  if (state.tab === "mrs") {
    children.push(txt((d.source_branch || "") + " → " + (d.target_branch || ""), { size: 9.5, muted: true, mono: true }));
    children.push(el("row", { gap: 4, children: [
      btn(d.approved ? "已批准" : "批准", "approve", null, true, !!d.approved),
      el("spacer")
    ] }));
    children.push(txt("评论", { size: 10, bold: true }));
    var notes = [];
    for (var i = 0; i < state.notes.length; i++) {
      var n = state.notes[i];
      notes.push({ text: "@" + (n.author ? n.author.username : "?") + ": " + (n.body || ""), detail: (n.created_at || "").slice(0, 10) });
    }
    children.push(el("list", { items: notes, scroll: true, maxHeight: 100 }));
    children.push(el("row", { gap: 4, children: [
      el("input", { key: "gl-comment", value: state.comment, placeholder: "添加评论…", action: "comment-input", grow: true }),
      btn("发送", "comment", null, true)
    ] }));
  }
  return el("column", { gap: 5, children: children });
}

function render(panelId) {
  var root = el("column", { grow: true, gap: 7, padding: 8, children: [] });
  root.children.push(tabs());
  if (state.loading) root.children.push(txt("正在连接 GitLab…", { size: 10, muted: true }));
  if (state.status) root.children.push(txt(state.status, { size: 10, tone: "ok" }));
  if (state.error) root.children.push(txt(state.error, { size: 10, tone: "err" }));
  if (state.showSettings) {
    root.children.push(settingsView());
    return root;
  }
  if (state.data.length === 0 && !state.loading) {
    root.children.push(txt("暂无数据，点击刷新重试", { size: 10.5, muted: true }));
    return root;
  }
  root.children.push(itemList());
  root.children.push(el("divider"));
  root.children.push(detailView());
  return root;
}
