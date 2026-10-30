// Nemorous SSH connection manager. Connections open in the app's embedded
// terminal, so ssh can prompt for host keys and passphrases normally.

var state = {
  profiles: [],
  selected: -1,
  editing: false,
  formKey: 0,
  confirmDelete: false,
  status: "",
  error: "",
  form: emptyProfile()
};

function emptyProfile() {
  return { name: "", host: "", user: "", port: "22", identity: "", remoteDir: "" };
}

function activate() {
  var saved = nemorous.storage.get("profiles");
  if (saved && saved.length) {
    state.profiles = saved;
    state.selected = 0;
    loadForm(0);
  } else {
    state.editing = true;
    state.form = emptyProfile();
  }
}

function saveProfiles() {
  nemorous.storage.set("profiles", state.profiles);
}

function loadForm(index) {
  state.selected = index;
  var p = state.profiles[index];
  state.form = p ? {
    name: p.name || "", host: p.host || "", user: p.user || "",
    port: p.port || "22", identity: p.identity || "", remoteDir: p.remoteDir || ""
  } : emptyProfile();
  state.editing = false;
  state.confirmDelete = false;
  state.formKey++;
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

function field(label, key, value, placeholder, width) {
  return el("row", { gap: 6, children: [
    txt(label, { size: 10, muted: true, width: 64 }),
    el("input", { key: "ssh-" + key + ":" + state.formKey, value: value, placeholder: placeholder || "", action: "field", grow: true, width: width || 0 })
  ] });
}

function shellQuote(value) {
  return "'" + String(value).replace(/'/g, "'\\''") + "'";
}

function commandFor(profile) {
  var args = ["ssh"];
  if (profile.port && profile.port !== "22") args.push("-p", profile.port);
  var identity = profile.identity || "";
  if (identity.indexOf("~/") === 0 || identity === "~") {
    var home = "";
    try { home = nemorous.home() || ""; } catch (e) {}
    if (home) identity = home + identity.slice(1);
  }
  if (identity) args.push("-i", identity);
  var target = (profile.user ? profile.user + "@" : "") + profile.host;
  if (profile.remoteDir) {
    args.push("-t", target, "cd " + shellQuote(profile.remoteDir) + " && exec $SHELL -l");
  } else {
    args.push(target);
  }
  return args;
}

function connect() {
  var p = state.form;
  if (!p.host || !p.user) {
    state.error = "请填写主机和用户名";
    return;
  }
  var workspace = nemorous.workspace();
  try {
    nemorous.terminal.open({
      command: commandFor(p),
      cwd: workspace.path || "",
      title: "SSH · " + p.user + "@" + p.host
    });
    state.status = "已打开 SSH 终端 · " + p.user + "@" + p.host;
    state.error = "";
  } catch (e) {
    state.error = String(e);
  }
}

function disconnect() {
  try {
    nemorous.terminal.close();
    state.status = "已断开";
    state.error = "";
  } catch (e) {
    state.error = String(e);
  }
}

function saveProfile() {
  var p = state.form;
  if (!p.host || !p.user) {
    state.error = "请填写主机和用户名";
    return;
  }
  if (!p.name) p.name = p.user + "@" + p.host;
  if (state.selected >= 0 && !state.editing) {
    state.profiles[state.selected] = p;
  } else {
    state.profiles.push(p);
    state.selected = state.profiles.length - 1;
  }
  state.editing = false;
  saveProfiles();
  state.status = "已保存 " + p.name;
  state.error = "";
}

function deleteProfile() {
  if (state.selected < 0) return;
  if (!state.confirmDelete) {
    state.confirmDelete = true;
    return;
  }
  state.profiles.splice(state.selected, 1);
  saveProfiles();
  state.confirmDelete = false;
  state.status = "已删除主机";
  if (state.profiles.length > 0) {
    loadForm(0);
  } else {
    state.selected = -1;
    state.editing = true;
    state.form = emptyProfile();
    state.formKey++;
  }
}

function handleAction(action, payload) {
  var p = payload || {};
  switch (action) {
    case "open":
      if (state.selected >= 0 && state.profiles[state.selected]) loadForm(state.selected);
      break;
    case "select":
      loadForm(p.index);
      break;
    case "new":
      state.selected = -1;
      state.editing = true;
      state.form = emptyProfile();
      state.formKey++;
      state.status = "";
      state.error = "";
      state.confirmDelete = false;
      break;
    case "field":
      if (p.key) {
        var key = String(p.key);
        if (key.indexOf("ssh-name:") === 0) state.form.name = String(p.value || "");
        else if (key.indexOf("ssh-host:") === 0) state.form.host = String(p.value || "");
        else if (key.indexOf("ssh-user:") === 0) state.form.user = String(p.value || "");
        else if (key.indexOf("ssh-port:") === 0) state.form.port = String(p.value || "");
        else if (key.indexOf("ssh-identity:") === 0) state.form.identity = String(p.value || "");
        else if (key.indexOf("ssh-remoteDir:") === 0) state.form.remoteDir = String(p.value || "");
      }
      break;
    case "save":
      saveProfile();
      break;
    case "delete":
      deleteProfile();
      break;
    case "connect":
      connect();
      break;
    case "disconnect":
      disconnect();
      break;
  }
  nemorous.invalidate();
}

function hostList() {
  var items = [];
  for (var i = 0; i < state.profiles.length; i++) {
    var p = state.profiles[i];
    items.push({
      text: p.name || (p.user + "@" + p.host),
      detail: p.host,
      action: "select",
      payload: { index: i },
      selected: i === state.selected && !state.editing
    });
  }
  return el("list", { items: items, maxHeight: 130, scroll: true });
}

function render(panelId) {
  var root = el("column", { grow: true, gap: 7, padding: 8, children: [] });
  root.children.push(el("row", { gap: 4, children: [
    btn("连接", "connect", null, true, !state.form.host || !state.form.user),
    btn("断开", "disconnect", null, false),
    btn("新增", "new", null, false),
    el("spacer")
  ] }));
  if (state.status) root.children.push(txt(state.status, { size: 10, tone: "ok" }));
  if (state.error) root.children.push(txt(state.error, { tone: "err", size: 10 }));
  if (state.profiles.length > 0) root.children.push(hostList());
  root.children.push(el("divider"));
  root.children.push(txt(state.editing ? "新建主机" : "编辑主机", { size: 10.5, bold: true }));
  root.children.push(field("名称", "name", state.form.name, "例如 production", 0));
  root.children.push(field("主机", "host", state.form.host, "git.internal.example.com", 0));
  root.children.push(field("用户", "user", state.form.user, "deploy", 0));
  root.children.push(field("端口", "port", state.form.port, "22", 90));
  root.children.push(field("私钥", "identity", state.form.identity, "~/.ssh/id_ed25519", 0));
  root.children.push(field("远程目录", "remoteDir", state.form.remoteDir, "/srv/app（可选）", 0));
  root.children.push(el("row", { gap: 4, children: [
    btn("保存", "save", null, true),
    btn(state.confirmDelete ? "确认删除" : "删除", "delete", null, false, state.selected < 0),
    el("spacer")
  ] }));
  return root;
}
