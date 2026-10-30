// Java tool window: scan main/test classes, run them through Maven, Gradle or
// a plain JVM, and expose a JDWP port for a remote debugger.
var MAX_OUTPUT = 200000;
var MAX_JAVA_FILES = 6000;
var scanTotal = 0;

var state = {
  loaded: false,
  stale: false,
  scanning: false,
  projects: [],
  selected: "",
  selectedMain: "",
  selectedTest: "",
  filter: "",
  view: "projects",
  output: "",
  status: "",
  statusTone: "muted",
  error: "",
  job: null,
  jobKind: "",
  java: "java",
  maven: "mvn",
  gradle: "gradle",
  debugPort: "5005",
  javaVersion: "",
  versionOutput: ""
};

function onWorkspaceChanged() {
  state.loaded = false;
  state.stale = true;
  state.scanning = false;
  state.projects = [];
  state.selected = "";
  state.selectedMain = "";
  state.selectedTest = "";
  state.output = "";
  state.error = "";
  state.status = "";
  state.statusTone = "muted";
  state.javaVersion = "";
  state.versionOutput = "";
}

function activate() {
  var saved = nemorous.storage.get("settings");
  if (saved && typeof saved === "object") {
    if (saved.java) state.java = saved.java;
    if (saved.maven) state.maven = saved.maven;
    if (saved.gradle) state.gradle = saved.gradle;
    if (saved.debugPort) state.debugPort = saved.debugPort;
  }
}

function saveSettings() {
  nemorous.storage.set("settings", {
    java: state.java,
    maven: state.maven,
    gradle: state.gradle,
    debugPort: state.debugPort
  });
}

// ---------- UI helpers ----------
function text(value, props) {
  var n = props || {};
  n.type = "text";
  n.text = String(value == null ? "" : value);
  return n;
}
function hint(value) { return text(value, { muted: true, size: 10.5 }); }
function badge(value, tone) { return { type: "badge", text: String(value), tone: tone || "" }; }
function btn(label, action, payload, props) {
  var n = props || {};
  n.type = "button";
  n.text = label;
  n.action = action;
  if (payload) n.payload = payload;
  return n;
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
  n.children = children.filter(function (c) { return !!c; });
  return n;
}
function col(children, props) {
  var n = props || {};
  n.type = "column";
  n.children = children.filter(function (c) { return !!c; });
  return n;
}
function list(items, props) {
  var n = props || {};
  n.type = "list";
  n.items = items;
  return n;
}
function spacer() { return { type: "spacer" }; }
function divider() { return { type: "divider" }; }

// ---------- filesystem helpers ----------
function baseName(p) {
  if (!p) return "";
  var v = String(p).replace(/[\\/]+$/, "");
  var i = Math.max(v.lastIndexOf("/"), v.lastIndexOf("\\"));
  return i >= 0 ? v.substring(i + 1) : v;
}
function relPath(p) {
  var ws = nemorous.workspace();
  var root = String(ws.path || "").replace(/\\/g, "/").replace(/\/+$/, "");
  var full = String(p || "").replace(/\\/g, "/");
  if (root && full.indexOf(root + "/") === 0) return full.substring(root.length + 1) || ".";
  return full;
}
function skipDir(name) {
  return name.charAt(0) === "." || name === "target" || name === "build" ||
    name === "out" || name === "node_modules" || name === ".gradle" ||
    name === "bin" || name === "dist" || name === ".idea";
}

function parseArtifact(pomPath) {
  var xml;
  try { xml = nemorous.fs.read(pomPath); } catch (e) { return { artifact: "", version: "", springBoot: false }; }
  xml = xml.replace(/<!--[\s\S]*?-->/g, "");
  var parent = xml.match(/<parent(?:\s[^>]*)?>[\s\S]*?<\/parent>/);
  var body = parent ? xml.replace(parent[0], "") : xml;
  var art = body.match(/<artifactId>\s*([^<]+?)\s*<\/artifactId>/);
  var ver = body.match(/<version>\s*([^<]+?)\s*<\/version>/);
  return {
    artifact: art ? art[1] : "",
    version: ver ? ver[1] : "",
    springBoot: /spring-boot-maven-plugin/.test(xml)
  };
}

function ensureProject(projects, path, repo) {
  for (var i = 0; i < projects.length; i++) {
    if (projects[i].path === path) return projects[i];
  }
  var p = {
    path: path,
    name: baseName(path),
    repoName: repo.name || "",
    kind: "Java",
    version: "",
    springBoot: false,
    mains: [],
    tests: [],
    files: 0
  };
  var pom = nemorous.fs.resolve(path, "pom.xml");
  if (nemorous.fs.exists(pom)) {
    p.kind = "Maven";
    var info = parseArtifact(pom);
    if (info.artifact) p.name = info.artifact;
    p.version = info.version;
    p.springBoot = info.springBoot;
  } else {
    var gradle = nemorous.fs.resolve(path, "build.gradle.kts");
    if (!nemorous.fs.exists(gradle)) gradle = nemorous.fs.resolve(path, "build.gradle");
    if (nemorous.fs.exists(gradle)) p.kind = "Gradle";
  }
  projects.push(p);
  return p;
}

function scanJavaFile(p, file, isTest, seen) {
  var name = file.name;
  if (!/\.java$/i.test(name) || file.size > 524288) return;
  if (scanTotal >= MAX_JAVA_FILES) return;
  scanTotal++;
  p.files++;
  var key = p.path + "|" + file.path;
  if (seen[key]) return;
  seen[key] = true;
  var source;
  try { source = nemorous.fs.read(file.path); } catch (e) { return; }
  var pm = source.match(/^\s*package\s+([\w.]+)\s*;/m);
  var pkg = pm ? pm[1] : "";
  var cls = name.replace(/\.java$/i, "");
  var fqcn = pkg ? pkg + "." + cls : cls;
  var testCount = (source.match(/@Test\b/g) || []).length;
  if (isTest || /\/src\/test\/java\//.test(file.path.replace(/\\/g, "/"))) {
    if (testCount > 0 || /(?:Test|Tests|IT)$/.test(cls)) {
      p.tests.push({
        className: fqcn,
        simpleName: cls,
        packageName: pkg,
        path: file.path,
        methods: testCount,
        project: p.path,
        projectName: p.name
      });
    }
    return;
  }
  var mainRE = /public\s+static\s+void\s+main\s*\(\s*(?:final\s+)?String\s*(?:\[\s*\]|\.\.\.|[A-Za-z_]\w*\s*\[\s*\])/;
  if (!mainRE.test(source)) return;
  p.mains.push({
    className: fqcn,
    simpleName: cls,
    packageName: pkg,
    path: file.path,
    project: p.path,
    projectName: p.name,
    springBoot: p.springBoot
  });
}

function scanJavaTree(p, dir, isTest, depth, seen) {
  if (depth > 16) return;
  var entries;
  try { entries = nemorous.fs.list(dir); } catch (e) { return; }
  for (var i = 0; i < entries.length; i++) {
    var e = entries[i];
    if (e.dir) {
      if (!skipDir(e.name)) scanJavaTree(p, e.path, isTest, depth + 1, seen);
    } else {
      scanJavaFile(p, e, isTest, seen);
    }
  }
}

function scanSourceRoot(projects, moduleRoot, repo, srcDir, isTest, seen) {
  var p = ensureProject(projects, moduleRoot, repo);
  scanJavaTree(p, srcDir, isTest, 0, seen);
}

function scanSources(projects, dir, repo, depth, seen) {
  if (depth > 10) return;
  var entries;
  try { entries = nemorous.fs.list(dir); } catch (e) { return; }
  var name = baseName(dir);
  if (name === "src") {
    var mainDir = nemorous.fs.resolve(dir, "main/java");
    var testDir = nemorous.fs.resolve(dir, "test/java");
    var moduleRoot = nemorous.fs.resolve(dir, "..");
    if (nemorous.fs.exists(mainDir)) scanSourceRoot(projects, moduleRoot, repo, mainDir, false, seen);
    if (nemorous.fs.exists(testDir)) scanSourceRoot(projects, moduleRoot, repo, testDir, true, seen);
    return;
  }
  for (var i = 0; i < entries.length; i++) {
    var e = entries[i];
    if (e.dir && !skipDir(e.name)) scanSources(projects, e.path, repo, depth + 1, seen);
  }
}

function refreshProjects() {
  if (state.scanning) return;
  state.scanning = true;
  state.error = "";
  nemorous.invalidate();
  var ws = nemorous.workspace();
  var repos = ws.repos && ws.repos.length ? ws.repos :
    [{ id: "workspace", name: baseName(ws.path) || "workspace", path: ws.path }];
  var projects = [];
  var seen = {};
  scanTotal = 0;
  for (var i = 0; i < repos.length; i++) {
    if (repos[i].path) scanSources(projects, repos[i].path, repos[i], 0, seen);
  }
  projects.sort(function (a, b) {
    if (a.repoName !== b.repoName) return a.repoName < b.repoName ? -1 : 1;
    return a.path < b.path ? -1 : (a.path > b.path ? 1 : 0);
  });
  var mains = 0, tests = 0;
  for (i = 0; i < projects.length; i++) {
    projects[i].mains.sort(function (a, b) { return a.className < b.className ? -1 : 1; });
    projects[i].tests.sort(function (a, b) { return a.className < b.className ? -1 : 1; });
    mains += projects[i].mains.length;
    tests += projects[i].tests.length;
  }
  state.projects = projects;
  state.loaded = true;
  state.stale = false;
  state.scanning = false;
  if (!findProject(state.selected)) {
    state.selected = projects.length ? projects[0].path : "";
  }
  var sel = selectedProject();
  if (!findMain(state.selectedMain)) {
    state.selectedMain = sel && sel.mains.length ? sel.path + "|" + sel.mains[0].className : "";
  }
  if (!findTest(state.selectedTest)) {
    state.selectedTest = sel && sel.tests.length ? sel.path + "|" + sel.tests[0].className : "";
  }
  state.status = projects.length ? "发现 " + projects.length + " 个 Java 模块 · " + mains + " 个主类 · " + tests + " 个测试类" : "未找到 Java 源码";
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
  return findProject(state.selected) || (state.projects.length ? state.projects[0] : null);
}
function findMain(key) {
  if (!key) return null;
  var parts = key.split("|");
  var p = findProject(parts[0]);
  if (!p) return null;
  for (var i = 0; i < p.mains.length; i++) if (p.mains[i].className === parts[1]) return p.mains[i];
  return null;
}
function selectedMainObj() { return findMain(state.selectedMain); }
function findTest(key) {
  if (!key) return null;
  var parts = key.split("|");
  var p = findProject(parts[0]);
  if (!p) return null;
  for (var i = 0; i < p.tests.length; i++) if (p.tests[i].className === parts[1]) return p.tests[i];
  return null;
}
function selectedTestObj() { return findTest(state.selectedTest); }
function allMains() {
  var out = [];
  for (var i = 0; i < state.projects.length; i++) {
    for (var j = 0; j < state.projects[i].mains.length; j++) out.push(state.projects[i].mains[j]);
  }
  return out;
}
function allTests() {
  var out = [];
  for (var i = 0; i < state.projects.length; i++) {
    for (var j = 0; j < state.projects[i].tests.length; j++) out.push(state.projects[i].tests[j]);
  }
  return out;
}

// ---------- commands ----------
function mavenExecutable(dir) {
  var chosen = (state.maven || "").trim();
  if (!chosen || chosen === "mvn") {
    var wrapper = findWrapper(dir, "mvnw");
    if (wrapper) return wrapper;
  }
  return chosen || "mvn";
}
function gradleExecutable(dir) {
  var chosen = (state.gradle || "").trim();
  if (!chosen || chosen === "gradle") {
    var wrapper = findWrapper(dir, "gradlew");
    if (wrapper) return wrapper;
  }
  return chosen || "gradle";
}
function safeExists(path) {
  try { return path && nemorous.fs.exists(path); } catch (e) { return false; }
}
function safeResolve(base, rel) {
  try { return nemorous.fs.resolve(base, rel); } catch (e) { return ""; }
}

function findWrapper(dir, name) {
  var d = dir;
  var wsPath = (nemorous.workspace().path || "").replace(/\/+$/, "");
  for (var guard = 0; d && guard < 12; guard++) {
    var path = safeResolve(d, name);
    if (safeExists(path)) return path;
    var cmd = safeResolve(d, name + ".cmd");
    if (safeExists(cmd)) return cmd;
    if (wsPath && d === wsPath) break;
    var parent = safeResolve(d, "..");
    if (!parent || parent === d) break;
    d = parent;
  }
  return "";
}

function debugAddress() {
  var port = (state.debugPort || "5005").trim();
  var major = parseInt((state.javaVersion || "").split(".")[0], 10);
  return "transport=dt_socket,server=y,suspend=n,address=" + (major >= 9 ? "*:" : "") + port;
}

function commandFor(main, debug) {
  var p = findProject(main.project);
  if (!p) return null;
  var dir = p.path;
  if (p.kind === "Maven") {
    var args = ["-B", "-q"];
    if (p.springBoot) {
      args.push("spring-boot:run");
    } else {
      args.push("compile", "exec:java", "-Dexec.mainClass=" + main.className);
    }
    return { command: mavenExecutable(dir), args: args, dir: dir };
  }
  if (p.kind === "Gradle") {
    return { command: gradleExecutable(dir), args: ["run"], dir: dir };
  }
  var java = (state.java || "java").trim() || "java";
  return {
    command: java,
    args: ["-cp", nemorous.fs.resolve(dir, "target/classes"), main.className],
    dir: dir
  };
}

function testCommand(test, all) {
  var p = findProject(test ? test.project : state.selected);
  if (!p) return null;
  if (p.kind === "Maven") {
    var args = ["-B", "test"];
    if (!all && test) args.push("-Dtest=" + test.simpleName);
    return { command: mavenExecutable(p.path), args: args, dir: p.path };
  }
  if (p.kind === "Gradle") {
    var ga = ["test"];
    if (!all && test) ga.push("--tests", test.className);
    return { command: gradleExecutable(p.path), args: ga, dir: p.path };
  }
  return null;
}

function quoteArg(a) {
  if (!/[^\w@%+=:,.\/-]/.test(a)) return a;
  return "'" + String(a).replace(/'/g, "'\\''") + "'";
}
function commandText(cmd) {
  if (!cmd) return "";
  var parts = [cmd.command || ""];
  for (var i = 0; i < (cmd.args || []).length; i++) parts.push(quoteArg(cmd.args[i]));
  return parts.join(" ");
}

function startJob(cmd, kind, extraEnv, label) {
  if (!cmd || !cmd.command) {
    nemorous.notify("warn", "无法确定运行命令");
    return;
  }
  if (state.job) {
    nemorous.notify("info", "已有 Java 任务正在运行");
    return;
  }
  state.output = "";
  state.error = "";
  state.jobKind = kind;
  state.status = label || "正在运行…";
  state.statusTone = "accent";
  nemorous.invalidate();
  var env = {};
  if (extraEnv) for (var k in extraEnv) if (extraEnv.hasOwnProperty(k)) env[k] = extraEnv[k];
  try {
    state.job = nemorous.exec({
      command: cmd.command,
      args: cmd.args || [],
      cwd: cmd.dir,
      env: Object.keys(env).length ? env : undefined,
      onOutput: function (chunk) {
        state.output = tailText(state.output + chunk, MAX_OUTPUT);
        nemorous.invalidate();
      },
      onExit: function (exitCode, message) {
        state.job = null;
        state.jobKind = "";
        if (kind === "version") {
          state.versionOutput = state.output;
          state.javaVersion = parseJavaVersion(state.output);
          state.status = state.javaVersion ? "JDK " + state.javaVersion : "JDK 检测完成";
          state.statusTone = exitCode === 0 ? "ok" : "err";
          return;
        }
        if (exitCode === 0) {
          state.status = kind === "test" ? "测试通过" : (kind === "debug" ? "调试进程已退出" : "运行成功");
          state.statusTone = "ok";
        } else {
          state.status = (kind === "test" ? "测试失败" : "运行失败") + (exitCode != null ? " (exit " + exitCode + ")" : "");
          if (message) state.error = message;
          state.statusTone = "err";
        }
        nemorous.invalidate();
      }
    });
  } catch (e) {
    state.job = null;
    state.jobKind = "";
    state.status = "无法启动 Java 进程";
    state.statusTone = "err";
    state.error = String(e);
    nemorous.invalidate();
  }
}

function tailText(s, max) {
  if (!s || s.length <= max) return s || "";
  return s.substring(s.length - max);
}
function parseJavaVersion(out) {
  if (!out) return "";
  var m = out.match(/version\s+"([^"]+)"/);
  if (m) return m[1];
  m = out.match(/openjdk\s+([0-9][^\s]*)/i);
  return m ? m[1] : "";
}

function runMain(debug) {
  var main = selectedMainObj();
  if (!main) {
    nemorous.notify("warn", "请先选择一个主类");
    return;
  }
  var cmd = commandFor(main, debug);
  if (!cmd) { nemorous.notify("err", "找不到主类所属模块"); return; }
  var env = {};
  if (debug) env.JAVA_TOOL_OPTIONS = "-agentlib:jdwp=" + debugAddress();
  startJob(cmd, debug ? "debug" : "run", env, debug ? "调试监听 " + state.debugPort + "…" : "正在启动 " + main.className + "…");
  state.view = "run";
}

function runTest(all) {
  var test = all ? null : selectedTestObj();
  var p = findProject(test ? test.project : state.selected);
  if (!p) { nemorous.notify("warn", "请先选择一个模块"); return; }
  var cmd = testCommand(test, all);
  if (!cmd) { nemorous.notify("warn", "当前模块不是 Maven/Gradle 项目，无法运行测试"); return; }
  startJob(cmd, "test", null, all ? "正在运行全部测试…" : "正在运行 " + test.className + "…");
  state.view = "tests";
}

function checkJava() {
  if (state.job) return;
  var java = (state.java || "java").trim() || "java";
  state.versionOutput = "";
  state.status = "正在检测 JDK…";
  state.statusTone = "accent";
  nemorous.invalidate();
  state.jobKind = "version";
  try {
    state.job = nemorous.exec({
      command: java,
      args: ["-version"],
      onOutput: function (chunk) {
        state.versionOutput = tailText(state.versionOutput + chunk, 20000);
        nemorous.invalidate();
      },
      onExit: function (code, message) {
        state.job = null;
        state.jobKind = "";
        state.javaVersion = parseJavaVersion(state.versionOutput);
        state.status = state.javaVersion ? "JDK " + state.javaVersion : (code === 0 ? "JDK 检测完成" : "找不到 Java");
        state.statusTone = code === 0 ? "ok" : "err";
        if (code !== 0 && message) state.error = message;
        nemorous.invalidate();
      }
    });
  } catch (e) {
    state.job = null;
    state.jobKind = "";
    state.status = "找不到 Java";
    state.statusTone = "err";
    state.error = String(e);
    nemorous.invalidate();
  }
}

// ---------- views ----------
function header() {
  var mains = allMains().length;
  var tests = allTests().length;
  return row([
    text("Java", { bold: true, size: 13 }),
    state.javaVersion ? badge("JDK " + state.javaVersion, "ok") : null,
    badge(state.projects.length + " 模块", "muted"),
    badge(mains + " 主类", mains ? "accent" : "muted"),
    badge(tests + " 测试", tests ? "" : "muted"),
    spacer(),
    btn("刷新", "refresh", null, { small: true, disabled: !!state.job }),
    btn("设置", "view", { value: "settings" }, { small: true, active: state.view === "settings" })
  ], { gap: 6 });
}

function tabs() {
  var defs = [["projects", "项目"], ["run", "运行"], ["tests", "测试"], ["settings", "设置"]];
  var children = [];
  for (var i = 0; i < defs.length; i++) {
    children.push(btn(defs[i][1], "view", { value: defs[i][0] }, { small: true, active: state.view === defs[i][0] }));
  }
  return row(children, { gap: 2 });
}

function render(panelID) {
  var children = [header(), tabs()];
  if (!state.loaded) {
    children.push(btn("扫描 Java 项目", "refresh", null, { primary: true }));
    children.push(hint("将扫描工作区中的 src/main/java 与 src/test/java。"));
  } else if (state.view === "run") {
    runView(children);
  } else if (state.view === "tests") {
    testsView(children);
  } else if (state.view === "settings") {
    settingsView(children);
  } else {
    projectsView(children);
  }
  if (state.scanning) children.push({ type: "spinner", text: "正在扫描 Java 源码…" });
  if (state.job && state.jobKind !== "version") {
    children.push(row([
      { type: "spinner", text: state.status || "运行中…" },
      spacer(),
      btn("停止", "cancel", null, { small: true })
    ], { gap: 4 }));
  } else if (state.status) {
    children.push(text(state.status, { muted: true, size: 9.5, tone: state.statusTone === "muted" ? "" : state.statusTone }));
  }
  if (state.error) children.push(text(state.error, { tone: "err", size: 9.5 }));
  return col(children, { grow: true, padding: 8, gap: 6 });
}

function filteredProjects() {
  var q = (state.filter || "").toLowerCase();
  if (!q) return state.projects;
  var out = [];
  for (var i = 0; i < state.projects.length; i++) {
    var p = state.projects[i];
    if ((p.name + " " + p.path + " " + p.repoName).toLowerCase().indexOf(q) >= 0) out.push(p);
  }
  return out;
}

function projectsView(children) {
  var projects = filteredProjects();
  var items = [];
  for (var i = 0; i < projects.length; i++) {
    items.push({
      id: projects[i].path,
      text: projects[i].name,
      detail: projects[i].kind + " · " + projects[i].mains.length + " 主类 · " + projects[i].tests.length + " 测试",
      selected: projects[i].path === state.selected,
      action: "selectProject",
      payload: { value: projects[i].path }
    });
  }
  var left = col([
    input("filter", state.filter, "搜索模块 / 路径", { action: "filter" }),
    list(items, { grow: true, scroll: true }),
    text(projects.length + " 个模块", { muted: true, size: 9.5 })
  ], { width: 290, heightPercent: 100, gap: 5 });
  var right = col(projectDetail(), { grow: true, heightPercent: 100, gap: 6 });
  children.push(row([left, { type: "divider", vertical: true }, right], { grow: true, gap: 6 }));
}

function projectDetail() {
  var p = selectedProject();
  if (!p) return [hint("未找到 Java 模块。")];
  var children = [];
  children.push(row([
    text(p.name, { bold: true, size: 12.5 }),
    badge(p.kind, p.kind === "Maven" ? "accent" : "muted"),
    p.version ? text("v" + p.version, { muted: true, size: 9.5, mono: true }) : null,
    p.springBoot ? badge("Spring Boot", "ok") : null,
    spacer(),
    text(relPath(p.path), { muted: true, size: 9.5 })
  ], { gap: 6 }));
  children.push(text(p.repoName + " · " + p.files + " 个 Java 文件", { muted: true, size: 9.5 }));
  children.push(row([
    badge(p.mains.length + " 主类", p.mains.length ? "accent" : "muted"),
    badge(p.tests.length + " 测试类", p.tests.length ? "ok" : "muted"),
    spacer(),
    (p.kind === "Maven" || p.kind === "Gradle") ? btn("运行模块测试", "runAllTests", null, { small: true, disabled: !!state.job }) : null
  ], { gap: 5 }));

  var mainItems = [];
  for (var i = 0; i < p.mains.length; i++) {
    var m = p.mains[i];
    mainItems.push({
      text: m.simpleName,
      detail: m.className,
      selected: p.path + "|" + m.className === state.selectedMain,
      action: "selectMain",
      payload: { value: p.path + "|" + m.className },
      mono: true
    });
  }
  children.push(text("主类", { bold: true, size: 10.5 }));
  if (mainItems.length) children.push(list(mainItems, { maxHeight: 220, scroll: true }));
  else children.push(hint("未发现 public static void main。"));

  var testItems = [];
  for (i = 0; i < p.tests.length; i++) {
    var t = p.tests[i];
    testItems.push({
      text: t.simpleName,
      detail: t.methods + " 个测试",
      selected: p.path + "|" + t.className === state.selectedTest,
      action: "selectTest",
      payload: { value: p.path + "|" + t.className },
      mono: true
    });
  }
  children.push(text("测试类", { bold: true, size: 10.5 }));
  if (testItems.length) children.push(list(testItems, { maxHeight: 180, scroll: true }));
  else children.push(hint("未发现 src/test/java 中的测试类。"));
  return children;
}

function runView(children) {
  var main = selectedMainObj();
  if (!main) {
    children.push(hint("还没有选择主类。先从项目页选择一个，或直接点击下面的入口。"));
    var items = [];
    var all = allMains();
    for (var i = 0; i < all.length && i < 30; i++) {
      var m = all[i];
      items.push({
        text: m.simpleName,
        detail: m.className + " · " + m.projectName,
        action: "selectMain",
        payload: { value: m.project + "|" + m.className },
        mono: true
      });
    }
    if (items.length) children.push(list(items, { maxHeight: 320, scroll: true }));
    else children.push(hint("先扫描项目，或在项目页选择一个主类。"));
    return;
  }
  var p = findProject(main.project);
  var cmd = commandFor(main, false);
  children.push(row([
    text(main.className, { bold: true, size: 12.5, mono: true }),
    p && p.springBoot ? badge("Spring Boot", "ok") : null,
    p ? badge(p.kind, "accent") : null,
    spacer(),
    text(p ? relPath(p.path) : "", { muted: true, size: 9.5 })
  ], { gap: 6 }));
  children.push(text(main.path, { muted: true, size: 9, mono: true }));
  children.push(row([
    btn("运行", "runMain", null, { primary: true, disabled: !!state.job }),
    btn("远程调试", "debugMain", null, { small: true, disabled: !!state.job }),
    spacer(),
    btn("项目", "view", { value: "projects" }, { small: true })
  ], { gap: 4 }));
  if (state.jobKind === "debug") {
    children.push(hint("JDWP 已开启：" + debugAddress() + "。用 IDEA / JVM 调试器连接 localhost:" + state.debugPort + "。"));
  }
  children.push(text("命令", { muted: true, size: 9.5 }));
  children.push(code(commandText(cmd), { maxHeight: 70, scroll: true }));
  appendOutput(children);
}

function testsView(children) {
  var test = selectedTestObj();
  if (!test) {
    children.push(hint("还没有选择测试类。"));
    var items = [];
    var all = allTests();
    for (var i = 0; i < all.length && i < 40; i++) {
      var t = all[i];
      items.push({
        text: t.simpleName,
        detail: t.className + " · " + t.projectName,
        action: "selectTest",
        payload: { value: t.project + "|" + t.className },
        mono: true
      });
    }
    if (items.length) children.push(list(items, { maxHeight: 320, scroll: true }));
    else children.push(hint("未发现测试类。"));
    return;
  }
  var p = findProject(test.project);
  children.push(row([
    text(test.className, { bold: true, size: 12.5, mono: true }),
    badge(test.methods + " 个测试", "ok"),
    p ? badge(p.kind, "accent") : null,
    spacer(),
    text(p ? relPath(p.path) : "", { muted: true, size: 9.5 })
  ], { gap: 6 }));
  children.push(text(test.path, { muted: true, size: 9, mono: true }));
  children.push(row([
    btn("运行此测试", "runTest", null, { primary: true, disabled: !!state.job }),
    btn("运行模块全部测试", "runAllTests", null, { small: true, disabled: !!state.job }),
    spacer(),
    btn("项目", "view", { value: "projects" }, { small: true })
  ], { gap: 4 }));
  children.push(text("命令", { muted: true, size: 9.5 }));
  children.push(code(commandText(testCommand(test, false)), { maxHeight: 70, scroll: true }));
  appendOutput(children);
}

function appendOutput(children) {
  children.push(divider());
  children.push(row([
    text("输出", { muted: true, size: 9.5 }),
    spacer(),
    state.output ? btn("清空", "clearOutput", null, { small: true }) : null
  ], { gap: 4 }));
  if (state.output) children.push(code(state.output, { grow: true, scroll: true, follow: true }));
  else children.push(hint("运行或测试时，进程输出会实时显示在这里。"));
}

function settingsView(children) {
  children.push(text("运行环境", { bold: true, size: 12 }));
  children.push(text("Java 可执行文件", { muted: true, size: 9.5 }));
  children.push(input("java", state.java, "java 或 /path/to/java"));
  children.push(text("Maven", { muted: true, size: 9.5 }));
  children.push(input("maven", state.maven, "mvn（优先使用项目 mvnw）"));
  children.push(text("Gradle", { muted: true, size: 9.5 }));
  children.push(input("gradle", state.gradle, "gradle（优先使用项目 gradlew）"));
  children.push(text("远程调试端口", { muted: true, size: 9.5 }));
  children.push(input("debugPort", state.debugPort, "5005"));
  children.push(row([
    btn("保存", "saveSettings", null, { primary: true, small: true }),
    btn("检测 JDK", "testJava", null, { small: true, disabled: !!state.job })
  ], { gap: 4 }));
  if (state.jobKind === "version") children.push({ type: "spinner", text: "正在运行 java -version…" });
  if (state.versionOutput) children.push(code(state.versionOutput, { maxHeight: 140, scroll: true }));
  children.push(divider());
  children.push(hint("权限：workspace.read（扫描 Java 源码）、process.exec（运行 Maven/Gradle/Java）。"));
  children.push(hint("Maven 与 Gradle 项目会优先使用仓库中的 wrapper；远程调试使用 JDWP，不内置调试器 UI。"));
  children.push(hint("工作区：" + (nemorous.workspace().path || "未打开")));
}

// ---------- actions ----------
function handleAction(action, payload, panelID) {
  payload = payload || {};
  switch (action) {
    case "open":
      if (!state.loaded || state.stale) refreshProjects();
      if (!state.javaVersion && !state.job) checkJava();
      return;
    case "refresh":
      if (state.job && state.jobKind !== "version") {
        nemorous.notify("info", "任务进行中，暂不能刷新");
        return;
      }
      refreshProjects();
      return;
    case "view":
      state.view = payload.value || "projects";
      break;
    case "selectProject":
      state.selected = payload.value || "";
      var p = selectedProject();
      state.selectedMain = p && p.mains.length ? p.path + "|" + p.mains[0].className : "";
      state.selectedTest = p && p.tests.length ? p.path + "|" + p.tests[0].className : "";
      state.view = "projects";
      break;
    case "selectMain":
      state.selectedMain = payload.value || "";
      state.view = "run";
      break;
    case "selectTest":
      state.selectedTest = payload.value || "";
      state.view = "tests";
      break;
    case "filter":
      state.filter = payload.value || "";
      break;
    case "runMain":
      runMain(false);
      return;
    case "debugMain":
      runMain(true);
      return;
    case "runTest":
      runTest(false);
      return;
    case "runAllTests":
      runTest(true);
      return;
    case "openTests":
      ensureLoaded();
      state.view = "tests";
      break;
    case "cancel":
      if (state.job) {
        nemorous.cancel(state.job);
        nemorous.notify("info", "已请求停止 Java 任务");
      }
      return;
    case "clearOutput":
      state.output = "";
      break;
    case "input":
      if (payload.key) state[payload.key] = payload.value;
      if (payload.key !== "filter") return;
      break;
    case "saveSettings":
      state.java = (state.java || "").trim() || "java";
      state.maven = (state.maven || "").trim() || "mvn";
      state.gradle = (state.gradle || "").trim() || "gradle";
      state.debugPort = (state.debugPort || "5005").trim() || "5005";
      saveSettings();
      nemorous.notify("ok", "Java 设置已保存");
      break;
    case "testJava":
      checkJava();
      return;
    default:
      return;
  }
  nemorous.invalidate();
}

function ensureLoaded() {
  if (!state.loaded || state.stale) refreshProjects();
}
