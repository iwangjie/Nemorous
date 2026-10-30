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
  filter: "",
  threads: "1C",
  jvmArgs: "",
  graph: null,
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
    if (typeof saved.threads === "string") {
      state.threads = saved.threads;
    }
    if (typeof saved.jvmArgs === "string") {
      state.jvmArgs = saved.jvmArgs;
    }
  }
}

function saveSettings() {
  nemorous.storage.set("settings", {
    mvn: state.mvn, extraArgs: state.extraArgs,
    threads: state.threads, jvmArgs: state.jvmArgs
  });
}

function onWorkspaceChanged() {
  state.loaded = false;
  state.scanning = false;
  state.stale = true;
  state.projects = [];
  state.roots = [];
  state.selected = "";
  state.graph = null;
  state.deps = [];
  state.depsFor = "";
  state.effPom = "";
  state.pomFor = "";
  state.error = "";
  if (state.job) {
    nemorous.cancel(state.job);
    state.job = null;
    state.jobKind = "";
  }
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
  n.action = n.action || "input";
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

function parseDependencyList(xml) {
  var out = [];
  var re = /<dependency(?:\s[^>]*)?>([\s\S]*?)<\/dependency>/g;
  var m;
  while ((m = re.exec(xml)) !== null) {
    var body = m[1];
    out.push({
      groupId: firstTag(body, "groupId"),
      artifactId: firstTag(body, "artifactId"),
      version: firstTag(body, "version"),
      scope: firstTag(body, "scope") || "compile",
      optional: firstTag(body, "optional") === "true",
      type: firstTag(body, "type") || "jar",
      classifier: firstTag(body, "classifier")
    });
  }
  return out;
}

function parseProperties(xml) {
  var props = {};
  var b = block(xml, "properties");
  if (!b) return props;
  var re = /<([A-Za-z0-9_.-]+)>([^<]*)<\/[A-Za-z0-9_.-]+>/g;
  var m;
  while ((m = re.exec(b.inner)) !== null) {
    props[m[1]] = m[2].replace(/^\s+|\s+$/g, "");
  }
  return props;
}

function resolveVersion(value, project) {
  var v = value || "";
  for (var i = 0; i < 4; i++) {
    var m = v.match(/^\$\{([^}]+)\}$/);
    if (!m) break;
    var key = m[1];
    if (key === "project.version" || key === "pom.version" || key === "version") v = project.version || "";
    else if (key === "project.groupId" || key === "pom.groupId") v = project.groupId || "";
    else if (key === "project.artifactId" || key === "pom.artifactId") v = project.artifactId || "";
    else v = project.properties[key] || "";
  }
  return v;
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
  var properties = parseProperties(xml);
  var management = block(xml, "dependencyManagement");
  var managed = management ? parseDependencyList(management.inner) : [];
  var dependenciesXml = management ? xml.replace(management.full, "") : xml;
  // Project-level dependencies only: plugin and profile dependencies are not reactor edges.
  var buildBlock = block(dependenciesXml, "build");
  if (buildBlock) dependenciesXml = dependenciesXml.replace(buildBlock.full, "");
  var profilesBlock = block(dependenciesXml, "profiles");
  if (profilesBlock) dependenciesXml = dependenciesXml.replace(profilesBlock.full, "");
  var reportingBlock = block(dependenciesXml, "reporting");
  if (reportingBlock) dependenciesXml = dependenciesXml.replace(reportingBlock.full, "");
  var directDeps = block(dependenciesXml, "dependencies");
  var dependencies = directDeps ? parseDependencyList(directDeps.inner) : [];
  var version = firstTag(body, "version");
  if (!version && parent) version = firstTag(parent.inner, "version");
  var parentGroup = parent ? firstTag(parent.inner, "groupId") : "";
  var parentArtifact = parent ? firstTag(parent.inner, "artifactId") : "";
  var project = {
    path: path,
    dir: dir,
    repoId: repo.id,
    repoName: repo.name,
    repoPath: repo.path,
    artifactId: artifactId,
    groupId: groupId,
    version: version,
    packaging: packaging,
    name: name,
    properties: properties,
    managed: managed,
    dependencies: dependencies,
    parentGroup: parentGroup,
    parentArtifact: parentArtifact,
    profileIds: [],
    modulePaths: [],
    children: [],
    parent: ""
  };
  project.version = resolveVersion(project.version, project);
  for (var di = 0; di < project.dependencies.length; di++) {
    project.dependencies[di].version = resolveVersion(project.dependencies[di].version, project);
  }
  var managedVersions = {};
  for (var mi = 0; mi < project.managed.length; mi++) {
    project.managed[mi].version = resolveVersion(project.managed[mi].version, project);
    managedVersions[project.managed[mi].groupId + ":" + project.managed[mi].artifactId] = project.managed[mi].version;
  }
  for (di = 0; di < project.dependencies.length; di++) {
    var dep = project.dependencies[di];
    if (!dep.version) {
      dep.version = managedVersions[dep.groupId + ":" + dep.artifactId] || "";
    }
  }

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
  if (depth > 12) return;
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
  buildDependencyGraph();
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

function buildDependencyGraph() {
  var projects = state.projects || [];
  var byCoord = {};
  var byArtifact = {};
  var byPath = {};
  var i, j;
  for (i = 0; i < projects.length; i++) {
    var p = projects[i];
    p.version = p.version || "?";
    p.deps = [];
    p.dependents = [];
    byPath[p.path] = p;
    if (p.groupId && p.artifactId) byCoord[p.groupId + ":" + p.artifactId] = p;
    // Artifact-only fallback is safe only when the id is unique; multi-repo
    // workspaces often contain unrelated modules with the same short name.
    if (p.artifactId) byArtifact[p.artifactId] = byArtifact[p.artifactId] ? null : p;
  }
  var edges = [];
  var conflictMap = {};
  for (i = 0; i < projects.length; i++) {
    p = projects[i];
    var key = (p.groupId || "?") + ":" + p.artifactId;
    if (!conflictMap[key]) conflictMap[key] = { groupId: p.groupId, artifactId: p.artifactId, versions: [], projects: [] };
    if (conflictMap[key].versions.indexOf(p.version) < 0) conflictMap[key].versions.push(p.version);
    conflictMap[key].projects.push(p);
    for (j = 0; j < p.dependencies.length; j++) {
      var dep = p.dependencies[j];
      var target = byCoord[(dep.groupId || "") + ":" + (dep.artifactId || "")] ||
        (dep.artifactId ? byArtifact[dep.artifactId] : null);
      if (!target || target === p) continue;
      p.deps.push(target);
      target.dependents.push(p);
      var mismatch = dep.version && dep.version !== "?" && !/^[\[(]/.test(dep.version) &&
        target.version && target.version !== "?" && dep.version !== target.version;
      edges.push({
        from: p,
        to: target,
        version: dep.version || "",
        scope: dep.scope || "compile",
        mismatch: mismatch,
        text: p.artifactId + "  →  " + target.artifactId,
        detail: (dep.version ? "声明 v" + dep.version + " · " : "") +
          (target.version ? "模块 v" + target.version + " · " : "") + (dep.scope || "compile")
      });
    }
  }
  // Build waves: dependencies first. A module enters the next wave when
  // every internal dependency is already placed.
  var placed = {};
  var waves = [];
  var cycle = null;
  var remaining = projects.slice(0);
  while (remaining.length > 0 && waves.length < 40) {
    var wave = [];
    var rest = [];
    for (i = 0; i < remaining.length; i++) {
      p = remaining[i];
      var ready = true;
      for (j = 0; j < p.deps.length; j++) {
        if (!placed[p.deps[j].path]) { ready = false; break; }
      }
      if (ready) wave.push(p);
      else rest.push(p);
    }
    if (wave.length === 0) {
      cycle = rest.slice(0);
      waves.push(rest);
      for (i = 0; i < rest.length; i++) placed[rest[i].path] = true;
      rest = [];
      break;
    }
    for (i = 0; i < wave.length; i++) placed[wave[i].path] = true;
    waves.push(wave);
    remaining = rest;
  }
  var conflicts = [];
  for (var ck in conflictMap) {
    if (!conflictMap.hasOwnProperty(ck)) continue;
    var c = conflictMap[ck];
    if (c.versions.length > 1) {
      conflicts.push({ text: c.groupId + ":" + c.artifactId, detail: "模块版本 " + c.versions.join(" / "), tone: "err" });
    }
  }
  for (i = 0; i < edges.length; i++) {
    if (edges[i].mismatch) {
      conflicts.push({
        text: edges[i].from.artifactId + " → " + edges[i].to.artifactId,
        detail: "声明 v" + edges[i].version + " · 模块 v" + edges[i].to.version,
        tone: "err"
      });
    }
  }
  if (cycle && cycle.length) {
    var names = [];
    for (i = 0; i < cycle.length; i++) names.push(cycle[i].artifactId);
    conflicts.push({ text: "循环依赖", detail: names.join(" ↔ "), tone: "err" });
  }
  var nodes = [];
  for (i = 0; i < projects.length; i++) {
    nodes.push({
      text: projects[i].artifactId,
      detail: (projects[i].version || "?") + " · " + (projects[i].repoName || ""),
      tone: projects[i].dependents.length > 0 ? "accent" : "muted",
      action: "select",
      payload: { path: projects[i].path }
    });
  }
  state.graph = {
    nodes: nodes,
    edges: edges,
    waves: waves,
    conflicts: conflicts
  };
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
  if (state.threads && state.threads !== "1" && state.threads !== "none") args.push("-T", state.threads);
  if (state.profile) args.push("-P" + state.profile);
  if (state.skipTests) args.push("-DskipTests");
  return args.concat(splitArgs(state.extraArgs));
}

function safeExists(path) {
  try { return path && nemorous.fs.exists(path); } catch (e) { return false; }
}
function safeResolve(base, rel) {
  try { return nemorous.fs.resolve(base, rel); } catch (e) { return ""; }
}

function mavenExecutable(dir) {
  var chosen = (state.mvn || "").trim();
  if (!chosen || chosen === "mvn") {
    var d = dir;
    var wsPath = (nemorous.workspace().path || "").replace(/\/+$/, "");
    for (var guard = 0; d && guard < 12; guard++) {
      var wrapper = safeResolve(d, "mvnw");
      if (safeExists(wrapper)) return wrapper;
      var cmd = safeResolve(d, "mvnw.cmd");
      if (safeExists(cmd)) return cmd;
      if (wsPath && d === wsPath) break;
      var parent = safeResolve(d, "..");
      if (!parent || parent === d) break;
      d = parent;
    }
  }
  return chosen || "mvn";
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
    var executable = mavenExecutable(dir);
    state.job = nemorous.exec({
      command: executable,
      args: args,
      cwd: dir,
      env: state.jvmArgs ? { MAVEN_OPTS: state.jvmArgs } : undefined,
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

function rootProject(p) {
  while (p && p.parent) {
    var parent = findProject(p.parent);
    if (!parent) break;
    p = parent;
  }
  return p;
}

function runChain(goal) {
  var p = selectedProject();
  if (!p) {
    nemorous.notify("warn", "请先选择一个 Maven 模块");
    return;
  }
  if (state.job) {
    nemorous.notify("info", "已有 Maven 任务正在运行");
    return;
  }
  var root = rootProject(p);
  if (!root || root.path === p.path) {
    runGoal(goal || "package");
    return;
  }
  syncProfiles();
  startMaven(baseArgs().concat(["-pl", ":" + p.artifactId, "-am", goal || "package"]), root.dir, "goal");
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
    return col(children, { grow: true, padding: 8, gap: 6 });
  }
  if (!state.loaded) {
    children.push(text("尚未加载 Maven 项目", { bold: true, size: 13 }));
    children.push(hint("Nemorous 会扫描工作区内所有 pom.xml（最多 6 层目录）。"));
    children.push(row([
      btn("扫描工作区", "refresh", null, { primary: true }),
      btn("设置", "settings", null, {})
    ], { gap: 6 }));
    if (state.error) children.push(text(state.error, { tone: "err", size: 11 }));
    return col(children, { grow: true, padding: 8, gap: 6 });
  }
  if (state.view === "graph") {
    graphView(children);
  } else if (state.view === "deps") {
    depsView(children);
  } else if (state.view === "pom") {
    pomView(children);
  } else if (state.view === "settings") {
    settingsView(children);
  } else {
    projectsView(children);
  }
  return col(children, { grow: true, padding: 8, gap: 6 });
}

function header() {
  var projects = state.projects ? state.projects.length : 0;
  return row([
    text("Maven", { bold: true, size: 13 }),
    text(projects + " 模块", { muted: true, size: 10 }),
    state.status ? badge(state.status, state.statusTone) : null,
    spacer(),
    state.job ? { type: "spinner", text: "构建中" } : null,
    btn("刷新", "refresh", null, { small: true, disabled: !!state.job }),
    btn("设置", "settings", null, { small: true })
  ], { gap: 6 });
}

function tabs() {
  var defs = [["projects", "构建"], ["graph", "依赖图"], ["deps", "依赖树"], ["pom", "POM"], ["settings", "设置"]];
  var items = [];
  for (var i = 0; i < defs.length; i++) {
    items.push(btn(defs[i][1], "view", { value: defs[i][0] }, {
      small: true,
      active: state.view === defs[i][0]
    }));
  }
  return row(items, { gap: 3 });
}

function flattenProject(p, depth, out, seen) {
  if (seen[p.path]) return;
  seen[p.path] = true;
  var rel = relPath(p);
  var detail = rel;
  if (depth === 0) detail = p.repoName + (rel === "." ? "" : " / " + rel);
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

function moduleItems() {
  var items = [];
  var seen = {};
  var filter = (state.filter || "").toLowerCase();
  for (var i = 0; i < state.roots.length; i++) {
    flattenProject(state.roots[i], 0, items, seen);
  }
  if (!filter) return items;
  var out = [];
  for (var j = 0; j < items.length; j++) {
    var item = items[j];
    var hay = (item.text + " " + item.detail).toLowerCase();
    if (hay.indexOf(filter) >= 0) {
      item.depth = 0;
      out.push(item);
    }
  }
  return out;
}

function projectsView(children) {
  if (!state.roots.length) {
    children.push(text("未找到 pom.xml", { bold: true, size: 12.5 }));
    children.push(hint("请确认工作区包含 Maven 项目，然后点击刷新。"));
    children.push(btn("重新扫描", "refresh", null, { primary: true }));
    return;
  }
  var items = moduleItems();
  var left = col([
    input("filter", state.filter, "搜索模块 / 路径", { action: "filter" }),
    { type: "list", items: items, grow: true, scroll: true, maxHeight: 520 },
    text(items.length + " 个模块", { muted: true, size: 9.5 })
  ], { width: 280, heightPercent: 100, gap: 5 });
  var right = col(buildPanel(), { grow: true, heightPercent: 100, gap: 6 });
  children.push(row([
    left,
    { type: "divider", vertical: true },
    right
  ], { grow: true, gap: 6 }));
}

function selectedMeta(p) {
  var deps = (state.graph && state.graph.edges) ? state.graph.edges : [];
  var out = 0, incoming = 0;
  for (var i = 0; i < deps.length; i++) {
    if (deps[i].from.path === p.path) out++;
    if (deps[i].to.path === p.path) incoming++;
  }
  return { out: out, incoming: incoming };
}

function buildPanel() {
  var p = selectedProject();
  if (!p) return [hint("从左侧选择一个模块。")];
  var meta = selectedMeta(p);
  var running = !!state.job;
  var children = [];
  children.push(row([
    text(p.artifactId, { bold: true, size: 12.5 }),
    badge(p.packaging || "jar", "muted"),
    text("v" + (p.version || "?"), { muted: true, size: 9.5, mono: true }),
    spacer(),
    text(relPath(p), { muted: true, size: 9.5 })
  ], { gap: 6 }));
  children.push(text(p.groupId + ":" + p.artifactId + " · " + p.repoName, { muted: true, size: 9.5, mono: true }));
  children.push(row([
    text("依赖 " + meta.out, { muted: true, size: 9.5 }),
    text("被 " + meta.incoming + " 个模块依赖", { muted: true, size: 9.5 }),
    spacer(),
    btn("查看依赖图", "view", { value: "graph" }, { small: true })
  ], { gap: 8 }));

  if (state.profiles.length) {
    var profileChildren = [text("Profile", { muted: true, size: 9.5 })];
    profileChildren.push(btn("无", "profile", { value: "" }, { small: true, active: state.profile === "" }));
    for (var j = 0; j < state.profiles.length; j++) {
      profileChildren.push(btn(state.profiles[j], "profile", { value: state.profiles[j] }, {
        small: true, active: state.profile === state.profiles[j]
      }));
    }
    children.push(row(profileChildren, { gap: 3 }));
  }

  children.push(row([
    btn("clean", "goal", { goal: "clean" }, { small: true, disabled: running }),
    btn("compile", "goal", { goal: "compile" }, { small: true, disabled: running }),
    btn("test", "goal", { goal: "test" }, { small: true, disabled: running }),
    btn("package", "goal", { goal: "package" }, { small: true, primary: true, disabled: running }),
    btn("install", "goal", { goal: "install" }, { small: true, disabled: running }),
    btn("含依赖构建", "buildChain", { goal: "package" }, { small: true, disabled: running || !state.graph })
  ], { gap: 3 }));
  children.push(row([
    input("customGoal", state.customGoal, "自定义目标，如 verify"),
    btn("运行", "goal", { goal: state.customGoal.trim() || "package" }, { small: true, disabled: running })
  ], { gap: 4 }));
  children.push(row([
    btn(state.skipTests ? "跳过测试 ✓" : "跳过测试", "skipTests", null, { small: true, active: state.skipTests }),
    btn("并发 " + (state.threads || "1"), "view", { value: "settings" }, { small: true, active: state.threads && state.threads !== "1" }),
    spacer(),
    running ? btn("停止", "cancel", null, { small: true }) : null,
    state.output ? btn("清空", "clearOutput", null, { small: true }) : null
  ], { gap: 4 }));
  if (state.error) children.push(text(state.error, { tone: "err", size: 10 }));
  children.push(divider());
  children.push(row([
    text("构建输出", { muted: true, size: 9.5 }),
    spacer(),
    state.jobKind ? badge(state.jobKind, "accent") : null
  ], { gap: 4 }));
  if (state.output) {
    children.push(code(outputTail(), { grow: true, scroll: true, follow: true }));
  } else {
    children.push(hint("选择生命周期目标后，实时输出会显示在这里。"));
    children.push(spacer());
  }
  return children;
}

function graphView(children) {
  var g = state.graph;
  if (!g || !state.projects.length) {
    children.push(hint("还没有可分析的 Maven 模块。"));
    return;
  }
  var conflictCount = g.conflicts.length;
  children.push(row([
    text("依赖关系图", { bold: true, size: 12 }),
    badge(g.nodes.length + " 模块", "muted"),
    badge(g.edges.length + " 依赖", "accent"),
    badge(g.waves.length + " 构建批次", "ok"),
    conflictCount ? badge(conflictCount + " 版本冲突", "err") : null,
    spacer(),
    btn("重新扫描", "refresh", null, { small: true, disabled: !!state.job })
  ], { gap: 5 }));
  children.push(hint("从左到右为构建批次；左侧模块先构建。箭头表示“依赖 → 被依赖”。"));

  var waveChildren = [];
  for (var i = 0; i < g.waves.length; i++) {
    var wave = g.waves[i];
    if (i > 0) waveChildren.push(text("→", { size: 18, muted: true }));
    var items = [];
    for (var j = 0; j < wave.length; j++) {
      var p = wave[j];
      items.push({
        text: p.artifactId,
        detail: "v" + (p.version || "?"),
        action: "select",
        payload: { path: p.path },
        selected: state.selected === p.path,
        bold: j === 0
      });
    }
    waveChildren.push(col([
      badge("批次 " + (i + 1), i === 0 ? "ok" : "accent"),
      { type: "list", items: items, maxHeight: 190, scroll: true }
    ], { width: 180, gap: 4 }));
  }
  children.push({ type: "row", children: waveChildren, gap: 8, scroll: true, padding: 2 });

  children.push(divider());
  children.push(text("依赖边", { muted: true, size: 9.5 }));
  if (!g.edges.length) {
    children.push(hint("没有跨模块内部依赖；这些模块彼此独立，可以并行构建。"));
  } else {
    var edgeItems = [];
    for (var k = 0; k < g.edges.length; k++) {
      edgeItems.push({
        text: g.edges[k].text,
        detail: g.edges[k].detail,
        mono: true,
        tone: g.edges[k].mismatch ? "err" : ""
      });
    }
    children.push({ type: "list", items: edgeItems, maxHeight: 150, scroll: true });
  }
  if (conflictCount) {
    children.push(divider());
    children.push(text("版本关联冲突", { tone: "err", size: 10, bold: true }));
    var conflictItems = [];
    for (var c = 0; c < g.conflicts.length; c++) {
      conflictItems.push({ text: g.conflicts[c].text, detail: "版本 " + g.conflicts[c].detail, tone: "err", mono: true });
    }
    children.push({ type: "list", items: conflictItems, maxHeight: 110, scroll: true });
  }
}

function depsView(children) {
  var p = selectedProject();
  children.push(row([
    text(p ? p.artifactId + " 依赖树" : "依赖树", { bold: true, size: 11.5 }),
    spacer(),
    btn("解析", "depsRefresh", null, { small: true, disabled: !!state.job })
  ], { gap: 4 }));
  if (state.jobKind === "deps") {
    children.push({ type: "spinner", text: "正在运行 dependency:tree…" });
  }
  if (!p) {
    children.push(hint("请先在「构建」中选择一个模块。"));
    return;
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
  children.push(text("构建设置", { bold: true, size: 12 }));
  children.push(text("Maven 可执行文件", { muted: true, size: 9.5 }));
  children.push(input("mvn", state.mvn, "mvn 或 /opt/homebrew/bin/mvn"));
  children.push(hint("留空或填 mvn 时优先使用项目根目录的 mvnw；也可填绝对路径固定版本。"));
  children.push(text("并发线程数（默认 1C）", { muted: true, size: 9.5 }));
  children.push(input("threads", state.threads, "1C / 4 / 1"));
  children.push(text("JVM 参数（MAVEN_OPTS）", { muted: true, size: 9.5 }));
  children.push(input("jvmArgs", state.jvmArgs, "-Xmx2g -XX:+UseZGC"));
  children.push(text("额外参数", { muted: true, size: 9.5 }));
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
  children.push(hint("并发构建默认开启（-T 1C），可按机器调整。"));
  children.push(hint("权限：workspace.read（读取 pom.xml）、process.exec（运行 Maven）。"));
  children.push(hint("工作区：" + (nemorous.workspace().path || "未打开")));
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
    case "buildChain":
      runChain(payload.goal || "package");
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
    case "openGraph":
      ensureProjects(function () {
        state.view = "graph";
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
    case "filter":
      state.filter = payload.value || "";
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
      if (payload.key !== "filter") return; // host keeps the text
      break;
    case "saveSettings":
      state.mvn = (state.mvn || "").trim() || "mvn";
      state.extraArgs = state.extraArgs || "";
      state.threads = (state.threads || "1C").trim() || "1C";
      state.jvmArgs = state.jvmArgs || "";
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
