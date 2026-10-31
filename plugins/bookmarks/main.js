// Bookmark manager: persistent bookmarks, categories, search, and automatic
// favicon discovery with a generated letter-avatar fallback.

var MAX_BOOKMARKS = 200;
var MAX_ICON_CHARS = 24000;

var state = {
  bookmarks: [],
  categories: [],
  category: "全部",
  query: "",
  view: "grid",
  editing: null,
  form: emptyForm(),
  formKey: 0,
  status: "",
  statusTone: "",
  error: "",
  confirmDelete: "",
  fetching: {},
  iconTimer: 0,
  iconQueue: [],
  iconForce: false
};

function emptyForm() {
  return { id: "", title: "", url: "", category: "", description: "", icon: "" };
}
function uid() { return "bm-" + Date.now() + "-" + Math.floor(Math.random() * 100000); }
function activate() {
  var saved = nemorous.storage.get("bookmarks");
  if (saved && saved.length) {
    state.bookmarks = saved.slice(0, MAX_BOOKMARKS).map(normalizeBookmark);
  } else {
    state.bookmarks = [];
  }
  rebuildCategories();
  state.iconTimer = setTimeout(refreshMissingIcons, 700);
}
function onWorkspaceChanged() {}
function save() {
  // The plugin store is capped at 1 MB. Keep every bookmark, but shed
  // favicon payloads from the oldest entries when the index gets close.
  try {
    if (JSON.stringify(state.bookmarks).length > 850000) {
      for (var i = state.bookmarks.length - 1; i >= 60; i--) {
        if (state.bookmarks[i].icon) state.bookmarks[i].icon = "";
        if (JSON.stringify(state.bookmarks).length <= 850000) break;
      }
    }
  } catch (e) {}
  nemorous.storage.set("bookmarks", state.bookmarks);
}
function normalizeBookmark(b) {
  return {
    id: b.id || uid(),
    title: String(b.title || ""),
    url: String(b.url || ""),
    category: String(b.category || ""),
    description: String(b.description || ""),
    icon: String(b.icon || ""),
    iconAt: Number(b.iconAt || 0),
    createdAt: Number(b.createdAt || Date.now()),
    lastOpened: Number(b.lastOpened || 0)
  };
}
function rebuildCategories() {
  var map = {};
  for (var i = 0; i < state.bookmarks.length; i++) {
    var c = state.bookmarks[i].category || "未分类";
    map[c] = (map[c] || 0) + 1;
  }
  var cats = [];
  for (var k in map) if (map.hasOwnProperty(k)) cats.push({ name: k, count: map[k] });
  cats.sort(function (a, b) { return a.name < b.name ? -1 : 1; });
  state.categories = cats;
  if (state.category !== "全部" && state.category !== "未分类" && !map[state.category]) state.category = "全部";
}

// ---------- UI helpers ----------
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
function baseName(p) {
  var v = String(p || "").replace(/[\\/]+$/, "");
  var i = Math.max(v.lastIndexOf("/"), v.lastIndexOf("\\"));
  return i >= 0 ? v.substring(i + 1) : v;
}
function hostOf(url) {
  var m = String(url || "").match(/^https?:\/\/([^\/]+)/i);
  return m ? m[1] : String(url || "");
}
function originOf(url) {
  var m = String(url || "").match(/^(https?:\/\/[^\/]+)/i);
  return m ? m[1] : "";
}
function dirOf(url) {
  var i = String(url || "").lastIndexOf("/");
  return i > 8 ? String(url).substring(0, i + 1) : String(url || "");
}
function normalizeURL(value) {
  var v = String(value || "").trim();
  if (!v) return "";
  if (!/^https?:\/\//i.test(v)) v = "https://" + v;
  var m = v.match(/^(https?):\/\/([^\s\/?#]+)([^\s]*)$/i);
  if (!m) return "";
  return m[1].toLowerCase() + "://" + m[2] + (m[3] || "/");
}
function headerValue(headers, name) {
  if (!headers) return "";
  var low = String(name).toLowerCase();
  for (var k in headers) if (headers.hasOwnProperty(k) && k.toLowerCase() === low) return String(headers[k] || "");
  return "";
}
function encodeB64(value) {
  var chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  var out = "";
  for (var i = 0; i < value.length; i += 3) {
    var a = value.charCodeAt(i) & 255;
    var b = i + 1 < value.length ? value.charCodeAt(i + 1) & 255 : NaN;
    var c = i + 2 < value.length ? value.charCodeAt(i + 2) & 255 : NaN;
    var n = (a << 16) | ((isNaN(b) ? 0 : b) << 8) | (isNaN(c) ? 0 : c);
    out += chars.charAt((n >> 18) & 63) + chars.charAt((n >> 12) & 63) +
      (isNaN(b) ? "=" : chars.charAt((n >> 6) & 63)) +
      (isNaN(c) ? "=" : chars.charAt(n & 63));
  }
  return out;
}
function encodeUTF8B64(s) {
  try { return encodeB64(unescape(encodeURIComponent(String(s)))); } catch (e) { return encodeB64(String(s)); }
}
function letterIcon(bm) {
  var host = hostOf(bm.url) || bm.title || "?";
  var letter = (bm.title || host).replace(/^www\./i, "").charAt(0).toUpperCase();
  var palette = ["#6366F1", "#0EA5E9", "#10B981", "#F59E0B", "#EF4444", "#8B5CF6", "#EC4899", "#14B8A6"];
  var sum = 0;
  for (var i = 0; i < host.length; i++) sum = (sum * 31 + host.charCodeAt(i)) & 0xffffffff;
  var color = palette[Math.abs(sum) % palette.length];
  var svg = '<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 48 48">' +
    '<rect width="48" height="48" rx="12" fill="' + color + '"/>' +
    '<text x="24" y="30" text-anchor="middle" font-family="Inter,Arial,sans-serif" font-size="22" font-weight="600" fill="#fff">' + escapeXML(letter) + '</text></svg>';
  return "data:image/svg+xml;base64," + encodeUTF8B64(svg);
}
function escapeXML(s) {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}
function currentList() {
  var q = String(state.query || "").toLowerCase();
  var out = [];
  for (var i = 0; i < state.bookmarks.length; i++) {
    var b = state.bookmarks[i];
    var cat = b.category || "未分类";
    if (state.category !== "全部" && cat !== state.category) continue;
    var hay = (b.title + " " + b.url + " " + b.description + " " + cat).toLowerCase();
    if (q && hay.indexOf(q) < 0) continue;
    out.push(b);
  }
  out.sort(function (a, b) { return (b.lastOpened || 0) - (a.lastOpened || 0) || (a.title < b.title ? -1 : 1); });
  return out;
}

// ---------- favicon ----------
function fetchIcon(bm, force) {
  if (!bm || !bm.url) return;
  if (state.fetching[bm.id]) return;
  state.fetching[bm.id] = true;
  var pageURL = bm.url;
  var origin = originOf(pageURL);
  var candidates = [];
  nemorous.http.request({
    method: "GET", url: pageURL, timeoutMs: 9000,
    onResponse: function (resp) {
      if (resp.ok && resp.body) {
        var base = dirOf(pageURL);
        var baseTag = String(resp.body).match(/<base[^>]+href=["']([^"']+)["']/i);
        if (baseTag) base = resolveURL(pageURL, baseTag[1]);
        var re = /<link\b[^>]*>/gi;
        var m;
        while ((m = re.exec(resp.body)) !== null) {
          var tag = m[0];
          if (!/rel=["'][^"']*icon/i.test(tag)) continue;
          var href = tag.match(/href=["']([^"']+)["']/i);
          if (href) candidates.push(resolveURL(base, href[1]));
        }
      }
      if (origin) candidates.push(origin + "/favicon.ico");
      var seen = {};
      var list = [];
      for (var i = 0; i < candidates.length; i++) {
        if (!seen[candidates[i]]) { seen[candidates[i]] = true; list.push(candidates[i]); }
      }
      tryIconList(bm, list, 0, force);
    },
    onError: function () {
      if (origin) tryIconList(bm, [origin + "/favicon.ico"], 0, force);
      else finishIcon(bm, force);
    }
  });
}
function resolveURL(base, href) {
  var h = String(href || "").trim();
  if (!h) return "";
  if (/^data:image\//i.test(h)) return h;
  if (/^https?:\/\//i.test(h)) return h;
  if (h.indexOf("//") === 0) {
    var m = base.match(/^(https?:)/i);
    return (m ? m[1] : "https:") + h;
  }
  if (h.charAt(0) === "/") return originOf(base) + h;
  return dirOf(base) + h.replace(/^\.\//, "");
}
function tryIconList(bm, urls, index, force) {
  if (index >= urls.length) { finishIcon(bm, force); return; }
  var u = urls[index];
  if (!u) { tryIconList(bm, urls, index + 1, force); return; }
  if (/^data:image\//i.test(u)) {
    if (u.length <= MAX_ICON_CHARS) { applyIcon(bm, u, force); return; }
    tryIconList(bm, urls, index + 1, force); return;
  }
  nemorous.http.request({
    method: "GET", url: u, timeoutMs: 8000,
    onResponse: function (resp) {
      if (!resp.ok || !resp.body) { tryIconList(bm, urls, index + 1, force); return; }
      var ct = headerValue(resp.headers, "content-type").toLowerCase();
      var body = String(resp.body);
      var data = "";
      if (ct.indexOf("svg") >= 0) {
        if (body.length <= 24000) data = "data:image/svg+xml;base64," + encodeUTF8B64(body);
      } else if (ct.indexOf("image/png") >= 0 || ct.indexOf("image/jpeg") >= 0 || ct.indexOf("image/gif") >= 0 || ct.indexOf("image/webp") >= 0) {
        if (body.length <= 20000) data = "data:" + ct.split(";")[0] + ";base64," + encodeB64(body);
      } else if (body.indexOf("<svg") >= 0 && body.length <= 24000) {
        data = "data:image/svg+xml;base64," + encodeUTF8B64(body);
      }
      if (data && data.length <= MAX_ICON_CHARS) applyIcon(bm, data, force);
      else tryIconList(bm, urls, index + 1, force);
    },
    onError: function () { tryIconList(bm, urls, index + 1, force); }
  });
}
function applyIcon(bm, data, force) {
  bm.icon = data;
  bm.iconAt = Date.now();
  delete state.fetching[bm.id];
  if (bm.id === "__form__") {
    state.form.icon = data;
    state.formKey++;
  } else {
    save();
  }
  state.status = "图标已更新 · " + bm.title;
  state.statusTone = "ok";
  nemorous.invalidate();
}
function finishIcon(bm, force) {
  if (!bm.icon) bm.icon = letterIcon(bm);
  bm.iconAt = Date.now();
  delete state.fetching[bm.id];
  if (bm.id === "__form__") {
    state.form.icon = bm.icon;
    state.formKey++;
  } else {
    save();
  }
  nemorous.invalidate();
}
function refreshMissingIcons() {
  var queue = [];
  var now = Date.now();
  for (var i = 0; i < state.bookmarks.length; i++) {
    var b = state.bookmarks[i];
    if (!b.icon || !b.iconAt || (now - b.iconAt > 30 * 86400000)) queue.push(b);
  }
  state.iconForce = false;
  state.iconQueue = queue.slice(0, 8);
  fetchNextIcon();
}
function refreshAllIcons() {
  if (state.iconTimer) clearTimeout(state.iconTimer);
  state.iconForce = true;
  state.iconQueue = state.bookmarks.slice(0);
  state.status = "正在刷新 " + state.iconQueue.length + " 个网站图标…";
  state.statusTone = "accent";
  nemorous.invalidate();
  fetchNextIcon();
}
function fetchNextIcon() {
  if (!state.iconQueue.length) {
    state.status = "图标刷新完成";
    state.statusTone = "ok";
    nemorous.invalidate();
    return;
  }
  var bm = state.iconQueue.shift();
  fetchIcon(bm, state.iconForce);
  state.iconTimer = setTimeout(fetchNextIcon, 500);
}

// ---------- views ----------
function header() {
  return row([
    text("Bookmarks", { bold: true, size: 13 }),
    badge(state.bookmarks.length + " 个", "muted"),
    state.categories.length ? badge(state.categories.length + " 分类", "muted") : null,
    spacer(),
    btn("刷新全部图标", "refreshIcons", null, { small: true }),
    btn("添加书签", "addBookmark", null, { primary: true, small: true })
  ], { gap: 5 });
}
function sidebar() {
  var all = state.bookmarks.length;
  var unclassified = 0;
  for (var i = 0; i < state.bookmarks.length; i++) if (!state.bookmarks[i].category) unclassified++;
  var items = [
    { text: "全部", detail: all + " 个", action: "category", payload: { value: "全部" }, selected: state.category === "全部" },
    { text: "未分类", detail: unclassified + " 个", action: "category", payload: { value: "未分类" }, selected: state.category === "未分类" }
  ];
  for (i = 0; i < state.categories.length; i++) {
    var c = state.categories[i];
    items.push({ text: c.name, detail: c.count + " 个", action: "category", payload: { value: c.name }, selected: state.category === c.name });
  }
  return col([
    text("分类", { bold: true, size: 10 }),
    list(items, { grow: true, scroll: true })
  ], { width: 165, heightPercent: 100, gap: 5 });
}
function bookmarkCard(b) {
  var host = hostOf(b.url);
  var icon = b.icon ? el("image", { src: b.icon, width: 28, height: 28 }) : el("image", { src: letterIcon(b), width: 28, height: 28 });
  var titleRow = row([
    icon,
    col([
      text(b.title, { bold: true, size: 11 }),
      text(host, { muted: true, size: 9, mono: true })
    ], { grow: true, gap: 1 })
  ], { gap: 6 });
  var children = [titleRow];
  if (b.description) children.push(text(b.description, { size: 9.5, muted: true }));
  children.push(row([
    badge(b.category || "未分类", b.category ? "accent" : "muted"),
    spacer(),
    btn("打开", "openBookmark", { id: b.id }, { small: true }),
    btn("编辑", "editBookmark", { id: b.id }, { small: true }),
    btn(state.confirmDelete === b.id ? "确认" : "删除", "deleteBookmark", { id: b.id }, { small: true, primary: state.confirmDelete === b.id })
  ], { gap: 3 }));
  return card(children, { padding: 10, gap: 5, widthPercent: 48 });
}
function gridView() {
  var listNow = currentList();
  var cards = [];
  for (var i = 0; i < listNow.length; i++) cards.push(bookmarkCard(listNow[i]));
  var children = [];
  children.push(row([
    input("filter", state.query, "搜索标题、网址、描述或分类", { action: "filter", grow: true }),
    state.query ? btn("清除", "clearFilter", null, { small: true }) : null
  ], { gap: 4 }));
  if (!cards.length) {
    children.push(card([
      text(state.bookmarks.length ? "没有匹配的书签" : "还没有书签", { bold: true, size: 12 }),
      hint(state.bookmarks.length ? "换个关键词或分类试试。" : "添加常用网站，支持自动抓取 favicon。"),
      btn("添加书签", "addBookmark", null, { primary: true })
    ], { padding: 12, gap: 6 }));
  } else {
    children.push(row(cards, { gap: 6 }));
  }
  return row([
    sidebar(),
    { type: "divider", vertical: true },
    col(children, { grow: true, heightPercent: 100, gap: 6, scroll: true })
  ], { grow: true, gap: 6 });
}
function editView() {
  var f = state.form;
  var children = [];
  var title = f.id ? "编辑书签" : "添加书签";
  var fields = [
    row([text(title, { bold: true, size: 12 }), spacer(), btn("返回", "back", null, { small: true })], { gap: 4 }),
    row([
      f.icon ? el("image", { src: f.icon, width: 42, height: 42 }) : el("image", { src: letterIcon(f), width: 42, height: 42 }),
      col([
        text("网站图标", { size: 9.5, muted: true }),
        row([
          btn("获取图标", "fetchFormIcon", null, { small: true }),
          btn("使用首字母", "letterIcon", null, { small: true })
        ], { gap: 3 })
      ], { gap: 3 })
    ], { gap: 8 }),
    field("标题", "title", f.title, "例如 GitHub"),
    field("网址", "url", f.url, "https://github.com"),
    field("分类", "category", f.category, "例如 开发 / 运维"),
    hint("输入新分类即可创建。"),
    categorySuggestions(),
    text("描述", { muted: true, size: 9.5 }),
    el("textarea", { key: "bm-desc:" + state.formKey, value: f.description, action: "field", height: 60, grow: true }),
    row([
      btn(f.id ? "保存修改" : "添加书签", "saveBookmark", null, { primary: true }),
      btn("取消", "back", null, { small: true })
    ], { gap: 4 })
  ];
  children.push(card(fields, { padding: 12, gap: 5 }));
  return children;
}
function categorySuggestions() {
  if (!state.categories.length) return null;
  var children = [text("已有分类", { muted: true, size: 9 })];
  for (var i = 0; i < state.categories.length && i < 10; i++) {
    children.push(btn(state.categories[i].name, "categoryForm", { value: state.categories[i].name }, { small: true, active: state.form.category === state.categories[i].name }));
  }
  return row(children, { gap: 3 });
}
function field(label, key, value, placeholder) {
  return row([
    text(label, { muted: true, size: 9.5, width: 52 }),
    input("bm-" + key + ":" + state.formKey, value, placeholder, { action: "field", grow: true })
  ], { gap: 4 });
}

// ---------- actions ----------
function findBookmark(id) {
  for (var i = 0; i < state.bookmarks.length; i++) if (state.bookmarks[i].id === id) return state.bookmarks[i];
  return null;
}
function startEdit(b) {
  state.editing = b ? b.id : "";
  state.form = b ? {
    id: b.id, title: b.title, url: b.url, category: b.category,
    description: b.description, icon: b.icon
  } : emptyForm();
  state.formKey++;
  state.view = "edit";
  state.confirmDelete = "";
}
function saveBookmark() {
  var f = state.form;
  var url = normalizeURL(f.url);
  if (!url) { state.error = "请填写有效的 http/https 网址"; return; }
  var title = f.title.trim() || hostOf(url) || url;
  var bm = findBookmark(f.id);
  if (!bm) {
    bm = { id: uid(), title: title, url: url, category: f.category.trim(), description: f.description.trim(), icon: f.icon || "", iconAt: 0, createdAt: Date.now(), lastOpened: 0 };
    state.bookmarks.unshift(bm);
  } else {
    bm.title = title; bm.url = url; bm.category = f.category.trim(); bm.description = f.description.trim();
    if (f.icon) bm.icon = f.icon;
  }
  if (!bm.icon) bm.icon = letterIcon(bm);
  state.bookmarks = state.bookmarks.slice(0, MAX_BOOKMARKS);
  rebuildCategories();
  save();
  state.view = "grid";
  state.status = f.id ? "书签已更新" : "书签已添加";
  state.statusTone = "ok";
  state.error = "";
  fetchIcon(bm, true);
  nemorous.invalidate();
}
function deleteBookmark(id) {
  if (state.confirmDelete !== id) {
    state.confirmDelete = id;
    nemorous.invalidate();
    return;
  }
  state.bookmarks = state.bookmarks.filter(function (b) { return b.id !== id; });
  state.confirmDelete = "";
  rebuildCategories();
  save();
  state.status = "书签已删除";
  state.statusTone = "ok";
  nemorous.invalidate();
}
function openBookmark(id) {
  var b = findBookmark(id);
  if (!b) return;
  b.lastOpened = Date.now();
  save();
  nemorous.openURL(b.url);
  nemorous.invalidate();
}
function fetchFormIcon() {
  var f = state.form;
  var url = normalizeURL(f.url);
  if (!url) { state.error = "请先填写有效网址"; return; }
  var temp = { id: "__form__", title: f.title || hostOf(url), url: url, icon: f.icon };
  state.fetching.__form__ = false;
  state.status = "正在获取网站图标…";
  state.statusTone = "accent";
  fetchIcon(temp, true);
  state.form.icon = temp.icon;
  state.formKey++;
}

function handleAction(action, payload, panelID) {
  var p = payload || {};
  switch (action) {
    case "open":
    case "openBookmarks":
      state.view = "grid";
      break;
    case "addBookmark":
      startEdit(null);
      return;
    case "editBookmark":
      startEdit(findBookmark(p.id));
      return;
    case "back":
      state.view = "grid";
      state.error = "";
      break;
    case "category":
      state.category = p.value || "全部";
      break;
    case "filter":
      state.query = String(p.value || "");
      break;
    case "clearFilter":
      state.query = "";
      break;
    case "field":
      var key = String(p.key || "").replace(/^bm-/, "");
      var fieldName = key.split(":")[0];
      if (fieldName === "title") state.form.title = String(p.value || "");
      else if (fieldName === "url") state.form.url = String(p.value || "");
      else if (fieldName === "category") state.form.category = String(p.value || "");
      else if (fieldName === "description" || fieldName === "desc") state.form.description = String(p.value || "");
      return;
    case "categoryForm":
      state.form.category = p.value || "";
      break;
    case "saveBookmark":
      saveBookmark();
      return;
    case "deleteBookmark":
      deleteBookmark(p.id);
      return;
    case "openBookmark":
      openBookmark(p.id);
      return;
    case "fetchFormIcon":
      fetchFormIcon();
      return;
    case "letterIcon":
      state.form.icon = letterIcon(state.form);
      state.formKey++;
      break;
    case "refreshIcons":
      refreshAllIcons();
      break;
    default:
      return;
  }
  nemorous.invalidate();
}

function render(panelID) {
  var children = [header()];
  if (state.status && state.view !== "edit") children.push(text(state.status, { size: 9.5, muted: true, tone: state.statusTone || "" }));
  if (state.error) children.push(text(state.error, { tone: "err", size: 9.5 }));
  if (state.view === "edit") {
    var form = editView();
    for (var i = 0; i < form.length; i++) children.push(form[i]);
  } else {
    children.push(gridView());
  }
  return col(children, { grow: true, padding: 8, gap: 6 });
}
