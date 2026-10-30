// Nemorous Maven plugin.
//
// The plugin is plain ES5: it runs in the host's embedded goja VM and
// draws its tool window through the declarative UI tree the host accepts
// from render(). All file access and process execution go through the
// `nemorous` host API, gated by the permissions declared in plugin.json.

var state = {
  view: "projects",
  loaded: false,
  scanning: false,
  stale: true,
  projects: [],
  roots: [],
  selected: "",
  profile: "",
  profiles: [],
  skipTests: false,
  mvn: "mvn",
  extraArgs: "",
  customGoal: "",
  output: "",
  status: "",
  statusTone: "",
  job: null,
  jobKind: "",
  deps: [],
  depsFor: "",
  effPom: "",
  pomFor: "",
  versionOutput: "",
  error: ""
};

var MAX_OUTPUT = 400000;

function activate() {
  var saved = nemorous.storage.get("settings");
  if (saved && typeof saved === "object") {
    if (typeof saved.mvn === "string" && saved.mvn) {
      state.mvn = saved.mvn;
    }
    if (typeof saved.extraArgs === "string") {
      state.extraArgs = saved.extraArgs;
    }
  }
}

function saveSettings() {
  nemorous.storage.set("settings", { mvn: state.mvn, extraArgs: state.extraArgs });
}

function onWorkspaceChanged() {
  state.loaded = false;
  state.scanning = false;
  state.stale = true;
  state.projects = [];
  state.roots = [];
  state.selected = "";
  state.deps = [];
  state.depsFor = "";
  state.effPom = "";
  state.pomFor = "";
  state.error = "";
  state.job = null;
  state.status = "";
  state.statusTone = "";
  state.output = "";
  nemorous.invalidate();
}

// ---------- helpers ----------

function el(type, props) {
  var n = props || {};
  n.type = type;
  return n;
}

function text(s, props) {
  var n = props || {};
  n.type = "text";
  n.text = String(s);
  return n;
}

function btn(label, action, payload, props) {
  var n = props || {};
  n.type = "button";
  n.text = label;
  n.action = action;
  if (payload !== null && payload !== undefined) {
    n.payload = payload;
  }
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

function clean(children) {
  var out = [];
  for (var i = 0; i < children.length; i++) {
    if (children[i]) {
      out.push(children[i]);
    }
  }
  return out;
}

function spacer() {
  return { type: "spacer" };
}

function divider() {
  return { type: "divider" };
}

function hint(s) {
  return text(s, { muted: true, size: 10.5 });
}

function badge(s, tone) {
  return { type: "badge", text: s, tone: tone || "" };
}

function input(key, value, placeholder, props) {
  var n = props || {};
  n.type = "input";
  n.key = key;
  n.value = value || "";
  n.placeholder = placeholder || "";
  n.action = "input";
  return n;
}

function code(text, props) {
  var n = props || {};
  n.type = "code";
  n.text = text;
  n.mono = true;
  return n;
}

function baseName(p) {
  if (!p) return "";
  var i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i >= 0 ? p.substring(i + 1) : p;
}

function relPath(p) {
  var repo = p.repoPath || "";
  var dir = p.dir || "";
  if (repo && dir.indexOf(repo) === 0) {
    var rel = dir.substring(repo.length);
    rel = rel.replace(/^[\/\\]+/, "");
    return rel === "" ? "." : rel;
  }
  return dir;
}

function tailText(s, max) {
  if (!s || s.length <= max) return s || "";
  return "…" + s.substring(s.length - max);
}

function outputTail() {
  var lines = state.output.split(/\r?\n/);
  if (lines.length > 600) {
    lines = lines.slice(lines.length - 600);
  }
  return lines.join("\n");
}

function splitArgs(s) {
  if (!s || !s.trim()) return [];
  var m = s.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) || [];
  var out = [];
  for (var i = 0; i < m.length; i++) {
    var a = m[i];
    if ((a.charAt(0) === '"' && a.charAt(a.length - 1) === '"') ||
        (a.charAt(0) === "'" && a.charAt(a.length - 1) === "'")) {
      a = a.substring(1, a.length - 1);
    }
    out.push(a);
  }
  return out;
}

// ---------- pom discovery ----------

function block(xml, tag) {
  var re = new RegExp("<" + tag + "(?:\\s[^>]*)?>([\\s\\S]*?)</" + tag + ">");
  var m = xml.match(re);
  if (!m) return null;
  return { full: m[0], inner: m[1] };
}

function firstTag(xml, tag) {
  var b = block(xml, tag);
  return b ? b.inner.replace(/^\s+|\s+$/g, "") : "";
}

function stripComments(xml) {
  return xml.replace(/<!--[\s\S]*?-->/g, "");
}

function parsePom(path, dir, repo) {
  var xml;
  try {
    xml = stripComments(nemorous.fs.read(path));
  } catch (e) {
    return null;
  }
  var parent = block(xml, "parent");
  var body = parent ? xml.replace(parent.full, "") : xml;
  var artifactId = firstTag(body, "artifactId");
  if (!artifactId) artifactId = baseName(dir);
  var groupId = firstTag(body, "groupId");
  if (!groupId && parent) groupId = firstTag(parent.inner, "groupId");
  var packaging = firstTag(body, "packaging") || "jar";
  var name = firstTag(body, "name") || artifactId;
  var project = {
    path: path,
    dir: dir,
    repoId: repo.id,
    repoName: repo.name,
    repoPath: repo.path,
    artifactId: artifactId,
    groupId: groupId,
    packaging: packaging,
    name: name,
    profileIds: [],
    modulePaths: [],
    children: [],
    parent: ""
  };

  var modulesBlock = block(xml, "modules");
  if (modulesBlock) {
    var moduleRe = /<module>\s*([^<]+?)\s*<\/module>/g;
    var mm;
    while ((mm = moduleRe.exec(modulesBlock.inner)) !== null) {
      var rel = mm[1].replace(/^\s+|\s+$/g, "").replace(/\\/g, "/");
      var modDir = nemorous.fs.resolve(dir, rel);
      var modPath = /\.xml$/i.test(modDir) ? modDir : nemorous.fs.resolve(modDir, "pom.xml");
      project.modulePaths.push(modPath);
    }
  }

  var profilesBlock = block(xml, "profiles");
  if (profilesBlock) {
    var profileRe = /<profile(?:\s[^>]*)?>([\s\S]*?)<\/profile>/g;
    var pm;
    while ((pm = profileRe.exec(profilesBlock.inner)) !== null) {
      var id = firstTag(pm[1], "id");
      if (id && project.profileIds.indexOf(id) < 0) {
        project.profileIds.push(id);
      }
    }
  }
  return project;
}

function skipDir(name) {
  return name.charAt(0) === "." || name === "target" ||
    name === "node_modules" || name === "build" || name === "out" ||
    name === ".gradle" || name === ".mvn";
}

function scanDir(dir, repo, depth, out) {
  if (depth > 6) return;
  var entries;
  try {
    entries = nemorous.fs.list(dir);
  } catch (e) {
    return;
  }
  var pom = null;
  var i;
  for (i = 0; i < entries.length; i++) {
    if (!entries[i].dir && entries[i].name === "pom.xml") {
      pom = entries[i];
      break;
    }
  }
  if (pom) {
    var project = parsePom(pom.path, dir, repo);
    if (project) out.push(project);
  }
  for (i = 0; i < entries.length; i++) {
    var e = entries[i];
    if (e.dir && !skipDir(e.name)) {
      scanDir(e.path, repo, depth + 1, out);
    }
  }
}

function refreshProjects() {
  if (state.scanning) return;
  state.scanning = true;
  state.error = "";
  nemorous.invalidate();
  var ws = nemorous.workspace();
  var projects = [];
  var repos = ws.repos && ws.repos.length ? ws.repos :
    [{ id: "workspace", name: baseName(ws.path) || "workspace", path: ws.path }];
  for (var i = 0; i < repos.length; i++) {
    if (!repos[i].path) continue;
    scanDir(repos[i].path, repos[i], 0, projects);
  }
  // A root repository's recursive scan can reach sibling repositories
  // inside the workspace; keep one project per pom path.
  var unique = [];
  var seenPath = {};
  for (i = 0; i < projects.length; i++) {
    if (!seenPath[projects[i].path]) {
      seenPath[projects[i].path] = true;
      unique.push(projects[i]);
    }
  }
  projects = unique;
  projects.sort(function (a, b) {
    if (a.repoName !== b.repoName) return a.repoName < b.repoName ? -1 : 1;
    return a.path < b.path ? -1 : (a.path > b.path ? 1 : 0);
  });
  var byPath = {};
  for (i = 0; i < projects.length; i++) {
    byPath[projects[i].path] = projects[i];
  }
  for (i = 0; i < projects.length; i++) {
    var p = projects[i];
    for (var j = 0; j < p.modulePaths.length; j++) {
      var child = byPath[p.modulePaths[j]];
      if (child && child !== p && !child.parent) {
        child.parent = p.path;
        p.children.push(child);
      }
    }
  }
  var roots = [];
  for (i = 0; i < projects.length; i++) {
    if (!projects[i].parent) roots.push(projects[i]);
  }
  state.projects = projects;
  state.roots = roots;
  state.scanning = false;
  state.loaded = true;
  state.stale = false;
  if (!findProject(state.selected)) {
    state.selected = roots.length ? roots[0].path : (projects.length ? projects[0].path : "");
  }
  syncProfiles();
  state.status = projects.length ? "发现 " + projects.length + " 个 Maven 项目" : "未找到 pom.xml";
  state.statusTone = projects.length ? "ok" : "warn";
  nemorous.invalidate();
}

function findProject(path) {
  for (var i = 0; i < state.projects.length; i++) {
    if (state.projects[i].path === path) return state.projects[i];
  }
  return null;
}

function selectedProject() {
  return findProject(state.selected);
}

function syncProfiles() {
  var p = selectedProject();
  state.profiles = p ? p.profileIds.slice(0) : [];
  if (state.profile && state.profiles.indexOf(state.profile) < 0) {
    state.profile = "";
  }
}

function ensureProjects(fn) {
  if (!state.loaded || state.stale) {
    refreshProjects();
  }
  fn();
}

// ---------- Maven execution ----------

function baseArgs() {
  var args = ["-B", "-ntp"];
  if (state.profile) args.push("-P" + state.profile);
  if (state.skipTests) args.push("-DskipTests");
  return args.concat(splitArgs(state.extraArgs));
}

function startMaven(args, dir, kind) {
  if (state.job) {
    nemorous.notify("info", "已有 Maven 任务正在运行");
    return;
  }
  state.output = "";
  state.error = "";
  state.status = "正在运行 Maven…";
  state.statusTone = "accent";
  state.jobKind = kind;
  nemorous.invalidate();
  try {
    state.job = nemorous.exec({
      command: state.mvn || "mvn",
      args: args,
      cwd: dir,
      onOutput: function (chunk) {
        state.output = tailText(state.output + chunk, MAX_OUTPUT);
        nemorous.invalidate();
      },
      onExit: function (code, message) {
        state.job = null;
        state.jobKind = "";
        var ok = code === 0;
        if (kind === "deps") {
          state.deps = ok ? parseDeps(state.output) : [];
          state.depsFor = state.selected;
        } else if (kind === "pom") {
          state.effPom = ok ? extractEffectivePom(state.output) : "";
          state.pomFor = state.selected;
        } else if (kind === "version") {
          state.versionOutput = tailText(state.output, 20000);
        }
        if (ok) {
          state.status = kind === "deps" ? "依赖树已更新" : (kind === "version" ? "Maven 可用" : "构建成功");
          state.statusTone = "ok";
        } else {
          state.status = "构建失败" + (message ? "：" + message : " (exit " + code + ")");
          state.statusTone = "err";
        }
        // A tab switched while a job ran waits for it; load its data now.
        if (state.view === "deps" && state.depsFor !== state.selected && kind !== "deps") {
          loadDeps();
          return;
        }
        if (state.view === "pom" && state.pomFor !== state.selected && kind !== "pom") {
          loadPom();
          return;
        }
        nemorous.invalidate();
      }
    });
  } catch (e) {
    state.job = null;
    state.jobKind = "";
    state.status = "无法运行 Maven：" + String(e);
    state.statusTone = "err";
    state.error = String(e);
    nemorous.invalidate();
  }
}

function runGoal(goal) {
  if (!goal) return;
  var p = selectedProject();
  if (!p) {
    nemorous.notify("warn", "请先选择一个 Maven 模块");
    return;
  }
  if (state.job) {
    nemorous.notify("info", "已有 Maven 任务正在运行");
    return;
  }
  syncProfiles();
  startMaven(baseArgs().concat([goal]), p.dir, "goal");
}

function loadDeps() {
  var p = selectedProject();
  if (!p) {
    nemorous.notify("warn", "请先选择一个 Maven 模块");
    return;
  }
  if (state.job) {
    nemorous.notify("info", "已有 Maven 任务正在运行");
    return;
  }
  state.view = "deps";
  state.deps = [];
  state.depsFor = "";
  startMaven(baseArgs().concat(["dependency:tree", "-DoutputType=text"]), p.dir, "deps");
}

function loadPom() {
  var p = selectedProject();
  if (!p) {
    nemorous.notify("warn", "请先选择一个 Maven 模块");
    return;
  }
  if (state.job) {
    nemorous.notify("info", "已有 Maven 任务正在运行");
    return;
  }
  state.view = "pom";
  state.effPom = "";
  state.pomFor = "";
  startMaven(baseArgs().concat(["help:effective-pom"]), p.dir, "pom");
}

function testMaven() {
  if (state.job) {
    nemorous.notify("info", "已有 Maven 任务正在运行");
    return;
  }
  var ws = nemorous.workspace();
  var dir = ws.path || (selectedProject() ? selectedProject().dir : "");
  if (!dir) {
    nemorous.notify("warn", "请先打开工作区");
    return;
  }
  state.versionOutput = "";
  startMaven(["-v"], dir, "version");
}

function parseDeps(output) {
  var lines = output.split(/\r?\n/);
  var items = [];
  var started = false;
  for (var i = 0; i < lines.length; i++) {
    var line = lines[i].replace(/^\[(INFO|WARNING|ERROR)\]\s?/, "");
    if (/^BUILD (SUCCESS|FAILURE)/.test(line) || /^Reactor Summary/.test(line)) {
      continue;
    }
    // Each reactor project starts a new tree with its coordinate.
    if (/^\S+:\S+:(jar|pom|war|ear|zip):/.test(line)) {
      started = true;
      items.push({ text: line, depth: 0, mono: true });
      continue;
    }
    if (!started) continue;
    var m = line.match(/^([| ]*)[+\\]- (.*)$/);
    if (m) {
      var prefix = m[1] || "";
      var depth = Math.floor(prefix.replace(/\|/g, "").length / 3) + 1;
      var coord = m[2];
      if (coord.indexOf("(version managed from") >= 0) continue;
      items.push({ text: coord, depth: depth, mono: true });
      if (items.length > 500) break;
    }
  }
  return items;
}

function extractEffectivePom(output) {
  var lines = output.split(/\r?\n/);
  var out = [];
  var started = false;
  for (var i = 0; i < lines.length; i++) {
    var line = lines[i].replace(/^\[(INFO|WARNING)\]\s?/, "");
    if (!started) {
      if (line.indexOf("<?xml") === 0 || line.indexOf("<project") === 0) {
        started = true;
      } else {
        continue;
      }
    }
    out.push(line);
  }
  if (!out.length) {
    return tailText(output, 200000);
  }
  return tailText(out.join("\n"), 400000);
}

// ---------- rendering ----------

function render(panelID) {
  var children = [header(), tabs()];
  if (state.scanning && !state.loaded) {
    children.push({ type: "spinner", text: "正在扫描工作区中的 pom.xml…" });
    return col(children, { grow: true, padding: 10, gap: 8 });
  }
  if (!state.loaded) {
    children.push(text("尚未加载 Maven 项目", { bold: true, size: 13 }));
    children.push(hint("Nemorous 会扫描工作区内所有 pom.xml（最多 6 层目录）。"));
    children.push(row([
      btn("扫描工作区", "refresh", null, { primary: true }),
      btn("设置", "settings", null, {})
    ], { gap: 6 }));
    if (state.error) {
      children.push(text(state.error, { tone: "err", size: 11 }));
    }
    return col(children, { grow: true, padding: 10, gap: 8 });
  }
  if (state.view === "deps") {
    depsView(children);
  } else if (state.view === "pom") {
    pomView(children);
  } else if (state.view === "settings") {
    settingsView(children);
  } else {
    projectsView(children);
  }
  return col(children, { grow: true, padding: 10, gap: 8 });
}

function header() {
  var projects = state.projects ? state.projects.length : 0;
  return row([
    text("Maven", { bold: true, size: 13 }),
    text(projects + " 个项目", { muted: true, size: 10 }),
    spacer(),
    btn("刷新", "refresh", null, { small: true, disabled: !!state.job })
  ], { gap: 6 });
}

function tabs() {
  var defs = [["projects", "项目"], ["deps", "依赖"], ["pom", "POM"], ["settings", "设置"]];
  var items = [];
  for (var i = 0; i < defs.length; i++) {
    items.push(btn(defs[i][1], "view", { value: defs[i][0] }, {
      small: true,
      active: state.view === defs[i][0]
    }));
  }
  return row(items, { gap: 4 });
}

function flattenProject(p, depth, out, seen) {
  if (seen[p.path]) return;
  seen[p.path] = true;
  var rel = relPath(p);
  var detail = rel;
  if (depth === 0) {
    detail = p.repoName + (rel === "." ? "" : " / " + rel);
  }
  out.push({
    text: p.artifactId,
    detail: detail,
    depth: depth,
    action: "select",
    payload: { path: p.path },
    selected: state.selected === p.path,
    bold: depth === 0
  });
  for (var i = 0; i < p.children.length; i++) {
    flattenProject(p.children[i], depth + 1, out, seen);
  }
}

function projectsView(children) {
  if (!state.roots.length) {
    children.push(text("未找到 pom.xml", { bold: true, size: 12.5 }));
    children.push(hint("请确认工作区包含 Maven 项目，然后点击刷新。"));
    children.push(btn("重新扫描", "refresh", null, { primary: true }));
    return;
  }
  var items = [];
  var seen = {};
  for (var i = 0; i < state.roots.length; i++) {
    flattenProject(state.roots[i], 0, items, seen);
  }
  children.push({ type: "list", items: items, maxHeight: 190, scroll: true });
  var p = selectedProject();
  if (!p) return;
  children.push(divider());
  children.push(row([
    text(p.artifactId, { bold: true, size: 12 }),
    text(p.packaging, { muted: true, size: 9.5 }),
    spacer(),
    text(relPath(p), { muted: true, size: 9.5 })
  ], { gap: 6 }));

  if (state.profiles.length) {
    var profileChildren = [text("Profile", { muted: true, size: 10 })];
    profileChildren.push(btn("无", "profile", { value: "" }, {
      small: true, active: state.profile === ""
    }));
    for (var j = 0; j < state.profiles.length; j++) {
      profileChildren.push(btn(state.profiles[j], "profile", { value: state.profiles[j] }, {
        small: true, active: state.profile === state.profiles[j]
      }));
    }
    children.push(row(profileChildren, { gap: 4 }));
  }

  var running = !!state.job;
  children.push(row([
    btn("clean", "goal", { goal: "clean" }, { small: true, disabled: running }),
    btn("compile", "goal", { goal: "compile" }, { small: true, disabled: running }),
    btn("test", "goal", { goal: "test" }, { small: true, disabled: running }),
    btn("package", "goal", { goal: "package" }, { small: true, primary: true, disabled: running }),
    btn("install", "goal", { goal: "install" }, { small: true, disabled: running })
  ], { gap: 3 }));
  children.push(row([
    input("customGoal", state.customGoal, "自定义目标，如 verify"),
    btn("运行", "goal", { goal: state.customGoal.trim() || "package" }, { small: true, disabled: running })
  ], { gap: 4 }));
  children.push(row([
    btn(state.skipTests ? "跳过测试 ✓" : "跳过测试", "skipTests", null, { small: true, active: state.skipTests }),
    spacer(),
    running ? btn("停止", "cancel", null, { small: true }) : null
  ], { gap: 4 }));
  if (state.status) {
    children.push(badge(state.status, state.statusTone));
  }
  if (state.error) {
    children.push(text(state.error, { tone: "err", size: 10.5 }));
  }
  children.push(divider());
  children.push(row([
    text("构建输出", { muted: true, size: 10 }),
    spacer(),
    state.output ? btn("清空", "clearOutput", null, { small: true }) : null
  ], { gap: 4 }));
  if (state.output) {
    children.push(code(outputTail(), { grow: true, scroll: true, follow: true }));
  } else {
    children.push(hint("运行一个生命周期目标后，输出会显示在这里。"));
  }
}

function depsView(children) {
  var p = selectedProject();
  children.push(row([
    text(p ? p.artifactId + " 依赖树" : "依赖树", { bold: true, size: 11.5 }),
    spacer(),
    btn("解析", "depsRefresh", null, { small: true, disabled: !!state.job })
  ], { gap: 4 }));
  if (state.status) {
    children.push(badge(state.status, state.statusTone));
  }
  if (!p) {
    children.push(hint("请先在「项目」中选择一个模块。"));
    return;
  }
  if (state.jobKind === "deps") {
    children.push({ type: "spinner", text: "正在运行 dependency:tree…" });
  }
  if (state.depsFor !== state.selected && !state.job) {
    children.push(hint("尚未解析该模块的依赖树。"));
    return;
  }
  if (!state.deps.length) {
    children.push(hint("没有依赖，或依赖树解析失败。"));
    return;
  }
  children.push({ type: "list", items: state.deps, grow: true, scroll: true });
}

function pomView(children) {
  var p = selectedProject();
  children.push(row([
    text(p ? p.artifactId + " Effective POM" : "Effective POM", { bold: true, size: 11.5 }),
    spacer(),
    btn("生成", "pomRefresh", null, { small: true, disabled: !!state.job })
  ], { gap: 4 }));
  if (state.status) {
    children.push(badge(state.status, state.statusTone));
  }
  if (state.jobKind === "pom") {
    children.push({ type: "spinner", text: "正在运行 help:effective-pom…" });
  }
  if (!state.effPom) {
    children.push(hint("生成合并父子 POM、profiles 后的 Effective POM。"));
    return;
  }
  children.push(code(state.effPom, { grow: true, scroll: true }));
}

function settingsView(children) {
  children.push(text("Maven 设置", { bold: true, size: 12 }));
  children.push(text("可执行文件", { muted: true, size: 10 }));
  children.push(input("mvn", state.mvn, "mvn 或 /opt/homebrew/bin/mvn"));
  children.push(text("额外参数", { muted: true, size: 10 }));
  children.push(input("extraArgs", state.extraArgs, "例如 -Dmaven.test.failure.ignore=true"));
  children.push(row([
    btn("保存", "saveSettings", null, { primary: true, small: true }),
    btn("验证 Maven", "testMaven", null, { small: true, disabled: !!state.job })
  ], { gap: 4 }));
  if (state.jobKind === "version") {
    children.push({ type: "spinner", text: "正在运行 mvn -v…" });
  }
  if (state.versionOutput) {
    children.push(code(state.versionOutput, { maxHeight: 140, scroll: true }));
  }
  children.push(divider());
  children.push(hint("插件权限：workspace.read（读取 pom.xml）、process.exec（运行 Maven）。"));
  children.push(hint("工作区：" + (nemorous.workspace().path || "未打开")));
  children.push(hint("插件版本：" + nemorous.plugin.version + " · Nemorous " + nemorous.appVersion));
}

// ---------- actions ----------

function handleAction(action, payload, panelID) {
  payload = payload || {};
  switch (action) {
    case "open":
      if (!state.loaded || state.stale) refreshProjects();
      return;
    case "refresh":
      if (state.job) {
        nemorous.notify("info", "构建进行中，暂不能刷新项目");
        return;
      }
      refreshProjects();
      return;
    case "select":
      state.selected = payload.path || "";
      state.deps = [];
      state.depsFor = "";
      state.effPom = "";
      state.pomFor = "";
      syncProfiles();
      break;
    case "goal":
      runGoal(payload.goal || "package");
      return;
    case "runPackage":
      ensureProjects(function () { runGoal("package"); });
      return;
    case "openDeps":
      ensureProjects(function () {
        state.view = "deps";
        loadDeps();
      });
      return;
    case "view":
      state.view = payload.value || "projects";
      if (state.view === "deps" && state.depsFor !== state.selected && !state.job) {
        loadDeps();
        return;
      }
      if (state.view === "pom" && state.pomFor !== state.selected && !state.job) {
        loadPom();
        return;
      }
      break;
    case "profile":
      state.profile = payload.value || "";
      syncProfiles();
      break;
    case "skipTests":
      state.skipTests = !state.skipTests;
      break;
    case "cancel":
      if (state.job) {
        nemorous.cancel(state.job);
        nemorous.notify("info", "已请求停止 Maven");
      }
      return;
    case "input":
      if (payload.key) state[payload.key] = payload.value;
      return; // the host keeps the text; no rerender needed per keystroke
    case "saveSettings":
      state.mvn = (state.mvn || "").trim() || "mvn";
      state.extraArgs = state.extraArgs || "";
      saveSettings();
      nemorous.notify("ok", "Maven 设置已保存");
      break;
    case "testMaven":
      testMaven();
      return;
    case "depsRefresh":
      loadDeps();
      return;
    case "pomRefresh":
      loadPom();
      return;
    case "clearOutput":
      state.output = "";
      state.status = "";
      state.statusTone = "";
      break;
    case "settings":
      state.view = "settings";
      break;
    default:
      break;
  }
  nemorous.invalidate();
}
