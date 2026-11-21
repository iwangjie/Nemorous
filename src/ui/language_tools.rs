use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;
use serde_json::{Value, json};

use super::workbench;
use crate::git::WorkspaceFile;
use crate::state::{self, CodeTab, app_signal};
use crate::workbench_lsp::{self as lsp, Edit, Position};
use crate::workbench_protocol::Connection;
use crate::workbench_run::split_args;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ServerKey {
    root: PathBuf,
    cmd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Document {
    id: String,
    uri: String,
    lang: &'static str,
    text: String,
    revision: u64,
}

#[derive(Default)]
struct Registry {
    servers: BTreeMap<ServerKey, Server>,
    failures: BTreeMap<ServerKey, String>,
}

struct Server {
    io: Connection,
    ready: bool,
    seq: u64,
    started: Instant,
    docs: BTreeMap<String, (i32, Document)>,
    requests: HashMap<u64, Pending>,
}

struct Pending {
    operation: Operation,
    document: Document,
    position: Value,
    started: Instant,
}

#[derive(Clone, Copy)]
enum Operation {
    Hover,
    Completion,
    Definition,
    References,
    Format,
    Actions,
    Rename,
    Hints,
}

impl Operation {
    fn method(self) -> &'static str {
        match self {
            Self::Hover => "textDocument/hover",
            Self::Completion => "textDocument/completion",
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
            Self::Format => "textDocument/formatting",
            Self::Actions => "textDocument/codeAction",
            Self::Rename => "textDocument/rename",
            Self::Hints => "textDocument/inlayHint",
        }
    }
}

#[derive(Clone, Default)]
struct Diagnostics(BTreeMap<String, Vec<Value>>);

fn enabled() -> bool {
    app_signal().with(|s| {
        s.cfg
            .extra
            .get("lsp_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

fn document(tab: &CodeTab, settings: Option<&Value>) -> Option<(ServerKey, Document)> {
    if !tab.revision.is_empty() || tab.binary || tab.too_large {
        return None;
    }
    let path = Path::new(&tab.file.abs_path);
    let (lang, default) = lsp::server(path)?;
    let cmd = settings
        .and_then(|value| value[lang].as_str())
        .unwrap_or(default);
    if cmd.trim().is_empty() {
        return None;
    }
    Some((
        ServerKey {
            root: PathBuf::from(&tab.file.repo_path),
            cmd: cmd.to_owned(),
        },
        Document {
            id: tab.id.clone(),
            uri: lsp::file_uri(path).ok()?,
            lang,
            text: tab.text.clone(),
            revision: tab.content_rev,
        },
    ))
}

impl Server {
    fn start(key: &ServerKey) -> Result<Self> {
        let args = split_args(&key.cmd)?;
        let (program, args) = args.split_first().context("LSP 命令为空")?;
        let mut cmd = Command::new(program);
        cmd.args(args).current_dir(&key.root);
        let io =
            Connection::stdio(&mut cmd).with_context(|| format!("无法启动语言服务器 {program}"))?;
        let uri = lsp::file_uri(&key.root)?;
        io.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "processId":std::process::id(),"rootUri":uri,
            "workspaceFolders":[{"uri":uri,"name":key.root.file_name()}],
            "capabilities":{"general":{"positionEncodings":["utf-16"]},
                "textDocument":{"synchronization":{"didSave":false},"completion":{"completionItem":{"snippetSupport":false}},
                "publishDiagnostics":{"versionSupport":true}},
                "workspace":{"workspaceFolders":true}},
        }}))?;
        Ok(Self {
            io,
            ready: false,
            seq: 1,
            started: Instant::now(),
            docs: BTreeMap::new(),
            requests: HashMap::new(),
        })
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.io
            .send(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }

    fn sync(&mut self, docs: &[Document]) {
        if !self.ready {
            return;
        }
        let removed = self
            .docs
            .keys()
            .filter(|uri| !docs.iter().any(|doc| doc.uri == **uri))
            .cloned()
            .collect::<Vec<_>>();
        for uri in removed {
            if self
                .notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
                .is_err()
            {
                return;
            }
            self.docs.remove(&uri);
            global::<Diagnostics>().update(|markers| {
                markers.0.remove(&uri);
            });
        }
        for doc in docs {
            let version = match self.docs.get(&doc.uri) {
                Some((_, previous)) if previous == doc => continue,
                Some((version, _)) => {
                    let next = version + 1;
                    if self.notify("textDocument/didChange",json!({
                        "textDocument":{"uri":doc.uri,"version":next},"contentChanges":[{"text":doc.text}]
                    })).is_err() { return; }
                    next
                }
                None => {
                    if self
                        .notify(
                            "textDocument/didOpen",
                            json!({"textDocument":{
                                "uri":doc.uri,"languageId":doc.lang,"version":1,"text":doc.text
                            }}),
                        )
                        .is_err()
                    {
                        return;
                    }
                    1
                }
            };
            self.docs.insert(doc.uri.clone(), (version, doc.clone()));
        }
    }

    fn poll(&mut self, answers: &mut Vec<(Pending, Value)>) -> Result<()> {
        ensure!(
            self.ready || self.started.elapsed() < Duration::from_secs(15),
            "语言服务器初始化超时"
        );
        self.requests
            .retain(|_, request| request.started.elapsed() < Duration::from_secs(20));
        for _ in 0..24 {
            let Some(message) = self.io.poll()? else {
                break;
            };
            let id = &message["id"];
            if let Some(method) = message["method"].as_str() {
                if !id.is_null() {
                    let reply = match method {
                        "workspace/configuration" => {
                            json!({"result": message["params"]["items"].as_array()
                            .map(|items| vec![Value::Null;items.len()]).unwrap_or_default()})
                        }
                        "workspace/workspaceFolders" => json!({"result":null}),
                        "client/registerCapability" | "window/workDoneProgress/create" => {
                            json!({"result":null})
                        }
                        _ => {
                            json!({"error":{"code":-32601,"message":"Unsupported client request"}})
                        }
                    };
                    let mut envelope = json!({"jsonrpc":"2.0","id":id});
                    envelope
                        .as_object_mut()
                        .unwrap()
                        .extend(reply.as_object().unwrap().clone());
                    self.io.send(envelope)?;
                } else if method == "textDocument/publishDiagnostics" {
                    let params = &message["params"];
                    if let Some(uri) = params["uri"].as_str() {
                        if self.docs.get(uri).is_some_and(|(version, _)| {
                            params["version"]
                                .as_i64()
                                .is_none_or(|actual| actual == i64::from(*version))
                        }) {
                            let diagnostics = params["diagnostics"]
                                .as_array()
                                .map(|items| items.iter().take(300).cloned().collect())
                                .unwrap_or_default();
                            global::<Diagnostics>().update(|markers| {
                                markers.0.insert(uri.into(), diagnostics);
                            });
                        }
                    }
                }
            } else if id == 1 && !self.ready {
                ensure!(message["error"].is_null(), "语言服务器拒绝初始化");
                let encoding = message["result"]["capabilities"]["positionEncoding"]
                    .as_str()
                    .unwrap_or("utf-16");
                ensure!(
                    encoding.eq_ignore_ascii_case("utf-16"),
                    "语言服务器选择了不受支持的坐标编码"
                );
                self.notify("initialized", json!({}))?;
                self.ready = true;
            } else if let Some(request) = id.as_u64().and_then(|id| self.requests.remove(&id)) {
                answers.push((request, message));
            }
        }
        Ok(())
    }
}

pub(super) fn setup() {
    let desired = computed(|| {
        if !enabled() {
            return Vec::new();
        }
        app_signal().with(|state| {
            state
                .code_tabs
                .iter()
                .filter_map(|tab| document(tab, state.cfg.extra.get("lsp_servers")))
                .collect::<Vec<_>>()
        })
    });
    effect(move || {
        let snapshot = desired.get();
        reconcile(&snapshot);
        if !snapshot.is_empty() {
            let timer = set_interval(Duration::from_millis(100), move || {
                reconcile(&desired.get_untracked())
            });
            cleanup(move || timer.cancel());
        }
    });
}

fn reconcile(snapshot: &[(ServerKey, Document)]) {
    let mut answers = Vec::new();
    workbench::update_resource::<Registry>(|registry| {
        registry.servers.retain(|key, server| {
            let keep = snapshot.iter().any(|(desired, _)| desired == key);
            if !keep {
                for uri in server.docs.keys() {
                    let _ =
                        server.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}));
                    global::<Diagnostics>().update(|markers| {
                        markers.0.remove(uri);
                    });
                }
            }
            keep
        });
        registry
            .failures
            .retain(|key, _| snapshot.iter().any(|(desired, _)| desired == key));
        for (key, _) in snapshot {
            if !registry.servers.contains_key(key) && !registry.failures.contains_key(key) {
                match Server::start(key) {
                    Ok(server) => {
                        registry.servers.insert(key.clone(), server);
                    }
                    Err(error) => {
                        registry.failures.insert(key.clone(), error.to_string());
                    }
                }
            }
        }
        let mut failed = Vec::new();
        for (key, server) in &mut registry.servers {
            let docs = snapshot
                .iter()
                .filter(|(current, _)| key == current)
                .map(|(_, doc)| doc.clone())
                .collect::<Vec<_>>();
            if let Err(error) = server.poll(&mut answers) {
                failed.push((key.clone(), error.to_string()));
            } else {
                server.sync(&docs);
            }
        }
        for (key, error) in failed {
            registry.servers.remove(&key);
            registry.failures.insert(key, error);
        }
    });
    for (pending, result) in answers {
        if let Err(error) = handle(pending, result) {
            state::show_toast("warn", error.to_string());
        }
    }
}

fn active_tab() -> Option<CodeTab> {
    app_signal().with_untracked(|s| s.code_tabs.get(s.active_code_tab_idx).cloned())
}

fn query(operation: Operation, new_name: Option<String>) -> Result<()> {
    ensure!(enabled(), "请先启用语言服务");
    let tab = active_tab().context("请打开源代码文件")?;
    let settings = app_signal().with_untracked(|s| s.cfg.extra.get("lsp_servers").cloned());
    let (key, doc) = document(&tab, settings.as_ref()).context("当前文件没有可用的语言服务")?;
    let mut answer = Ok(());
    workbench::update_resource::<Registry>(|registry| {
        answer = (|| {
            let server = registry.servers.get_mut(&key).with_context(|| {
                registry
                    .failures
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| "语言服务器正在启动".into())
            })?;
            ensure!(
                server.ready
                    && server
                        .docs
                        .get(&doc.uri)
                        .is_some_and(|(_, current)| current == &doc),
                "语言服务正在同步文档"
            );
            ensure!(server.requests.len() < 30, "语言服务请求过多");
            let line = tab
                .text
                .split('\n')
                .nth(tab.cursor.line as usize)
                .unwrap_or_default();
            let column = line
                .chars()
                .take(tab.cursor.column as usize)
                .map(char::len_utf16)
                .sum::<usize>();
            let position = json!({"line":tab.cursor.line,"character":column});
            let mut params = json!({"textDocument":{"uri":doc.uri},"position":position});
            match operation {
                Operation::Format => {
                    params["options"] = json!({"tabSize":4,"insertSpaces":true});
                }
                Operation::References => {
                    params["context"] = json!({"includeDeclaration":true});
                }
                Operation::Rename => {
                    params["newName"] = json!(new_name);
                }
                Operation::Actions => {
                    params["range"] = json!({"start":position,"end":position});
                    params["context"] = json!({"diagnostics":global::<Diagnostics>().with_untracked(|marks|
                        marks.0.get(&doc.uri).cloned().unwrap_or_default())});
                }
                Operation::Hints => {
                    let end = tab.text.split('\n').count().saturating_sub(1);
                    let column = tab
                        .text
                        .split('\n')
                        .next_back()
                        .unwrap_or_default()
                        .encode_utf16()
                        .count();
                    params["range"] = json!({"start":{"line":0,"character":0},"end":{"line":end,"character":column}});
                }
                _ => {}
            }
            server.seq += 1;
            server.io.send(json!({"jsonrpc":"2.0","id":server.seq,"method":operation.method(),"params":params}))?;
            server.requests.insert(
                server.seq,
                Pending {
                    operation,
                    document: doc,
                    position,
                    started: Instant::now(),
                },
            );
            Ok(())
        })();
    });
    answer
}

fn request(operation: Operation) {
    if let Err(error) = query(operation, None) {
        state::show_toast("warn", error.to_string());
    }
}

fn unchanged(doc: &Document) -> bool {
    app_signal().with_untracked(|s| {
        s.code_tabs
            .iter()
            .any(|tab| tab.id == doc.id && tab.content_rev == doc.revision && tab.text == doc.text)
    })
}

fn approved_edits(doc: &Document, changes: Vec<(String, Option<i32>, Vec<Edit>)>) -> Result<()> {
    ensure!(unchanged(doc), "LSP 结果已经过期");
    let mut plan = Vec::new();
    for (uri, version, edits) in changes {
        let path = lsp::uri_path(&uri)?;
        let (file, open) = app_signal().with_untracked(|s| {
            let file = s
                .workspace_files
                .iter()
                .find(|file| Path::new(&file.abs_path) == path)
                .cloned();
            let tab = s
                .code_tabs
                .iter()
                .find(|tab| tab.revision.is_empty() && Path::new(&tab.file.abs_path) == path)
                .cloned();
            (file, tab)
        });
        let file = file
            .or_else(|| open.as_ref().map(|tab| tab.file.clone()))
            .context("LSP 编辑超出当前工作区")?;
        if let Some(version) = version {
            let current = workbench::with_resource::<Registry, _>(|registry| {
                registry.servers.values().find_map(|server| {
                    server
                        .docs
                        .get(&uri)
                        .map(|(version, doc)| (*version, doc.revision))
                })
            });
            ensure!(
                current.is_some_and(|(current, rev)| current == version
                    && open.as_ref().is_some_and(|tab| tab.content_rev == rev)),
                "LSP 编辑版本已过期"
            );
        }
        let original = if let Some(open) = open {
            ensure!(open.editable, "LSP 目标文件为只读");
            open.text
        } else {
            let physical = path.canonicalize()?;
            ensure!(
                physical.starts_with(Path::new(&file.repo_path).canonicalize()?),
                "LSP 文件路径超出仓库"
            );
            ensure!(
                physical.metadata()?.len() as usize <= crate::git::MAX_REVIEW_BYTES,
                "LSP 文件超过大小限制"
            );
            std::fs::read_to_string(physical)?.replace("\r\n", "\n")
        };
        let replacement = lsp::apply_edits(&original, &edits)?;
        plan.push((file, original, replacement));
    }
    if plan.is_empty() {
        return Ok(());
    }
    let reviewed = doc.clone();
    super::git_prompt::confirm(
        format!("将语言服务建议应用到 {} 个文件的编辑器缓冲？", plan.len()),
        move || {
            if !unchanged(&reviewed) {
                state::show_toast("warn", "源文件已变化，已取消应用语言服务编辑");
                return;
            }
            let mut writes = Vec::new();
            for (file, original, new) in &plan {
                state::open_code_file(file.clone(), None, false);
                let tab = app_signal().with_untracked(|s| {
                    s.code_tabs
                        .iter()
                        .find(|tab| tab.revision.is_empty() && tab.file.abs_path == file.abs_path)
                        .cloned()
                });
                let Some(tab) = tab
                    .filter(|tab| tab.editable && &tab.text == original && !tab.external_changed)
                else {
                    state::show_toast("warn", "文件已变化，已取消应用语言服务编辑");
                    return;
                };
                writes.push((tab.id, new.clone()));
            }
            for (id, text) in writes {
                crate::code_split_runtime::record_editor_change(&id, &text);
            }
            state::show_toast("ok", "已应用到编辑器，可审阅后保存");
        },
    );
    Ok(())
}

fn location(value: &Value) -> Option<(WorkspaceFile, Position)> {
    let path = lsp::uri_path(
        value["uri"]
            .as_str()
            .or_else(|| value["targetUri"].as_str())?,
    )
    .ok()?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))?;
    let position = serde_json::from_value(range["start"].clone()).ok()?;
    let file = app_signal().with_untracked(|s| {
        s.workspace_files
            .iter()
            .find(|file| Path::new(&file.abs_path) == path)
            .cloned()
    })?;
    Some((file, position))
}

fn goto(file: WorkspaceFile, position: Position) {
    let source = app_signal()
        .with_untracked(|s| {
            s.code_tabs
                .iter()
                .find(|tab| tab.file.abs_path == file.abs_path)
                .map(|tab| tab.text.clone())
        })
        .or_else(|| std::fs::read_to_string(&file.abs_path).ok())
        .unwrap_or_default();
    if let Ok(position) = lsp::editor_position(&source, position) {
        state::open_code_file(file, Some(position), true);
    }
}

fn message_text(value: &Value) -> String {
    if let Some(value) = value.as_str() {
        value.into()
    } else if let Some(value) = value["value"].as_str() {
        value.into()
    } else if let Some(values) = value.as_array() {
        values
            .iter()
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    }
}

fn handle(request: Pending, message: Value) -> Result<()> {
    let doc = &request.document;
    ensure!(unchanged(doc), "语言服务结果已过期");
    ensure!(
        message["error"].is_null(),
        "语言服务请求失败 · {}",
        message["error"]["message"].as_str().unwrap_or_default()
    );
    let result = &message["result"];
    if result.is_null() {
        state::show_toast("info", "语言服务无结果");
        return Ok(());
    }
    match request.operation {
        Operation::Hover => {
            super::git_advanced::report("悬停信息".into(), message_text(&result["contents"]))
        }
        Operation::Completion => {
            let options = result.as_array().or_else(|| result["items"].as_array());
            let mut rows = Vec::new();
            for item in options.into_iter().flatten().take(50) {
                let Some(label) = item["label"].as_str() else {
                    continue;
                };
                if item["insertTextFormat"] == 2 {
                    continue;
                }
                let edit: Edit = match serde_json::from_value(item["textEdit"].clone()) {
                    Ok(edit) => edit,
                    Err(_) => {
                        let position: Position = serde_json::from_value(request.position.clone())?;
                        Edit {
                            range: lsp::Range {
                                start: position,
                                end: position,
                            },
                            text: item["insertText"].as_str().unwrap_or(label).to_owned(),
                        }
                    }
                };
                let doc = doc.clone();
                rows.push(MenuItem::new(label).on_select(move || {
                    if let Err(error) =
                        approved_edits(&doc, vec![(doc.uri.clone(), None, vec![edit.clone()])])
                    {
                        state::show_toast("warn", error.to_string());
                    }
                }));
            }
            if !rows.is_empty() {
                state::show_popup_menu(rows);
            }
        }
        Operation::Definition | Operation::References => {
            let result = result
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![result.clone()]);
            let rows = result
                .iter()
                .filter_map(location)
                .take(70)
                .map(|(file, position)| {
                    MenuItem::new(format!("{}:{}", file.rel_path, position.line + 1))
                        .on_select(move || goto(file.clone(), position))
                })
                .collect::<Vec<_>>();
            if !rows.is_empty() {
                state::show_popup_menu(rows);
            }
        }
        Operation::Format => approved_edits(
            doc,
            vec![(
                doc.uri.clone(),
                None,
                serde_json::from_value(result.clone())?,
            )],
        )?,
        Operation::Rename => approved_edits(doc, lsp::workspace_edits(result)?)?,
        Operation::Actions => {
            let rows = result
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|action| {
                    let label = action["title"].as_str()?;
                    let changes = lsp::workspace_edits(action.get("edit")?).ok()?;
                    let doc = doc.clone();
                    Some(MenuItem::new(label).on_select(move || {
                        if let Err(error) = approved_edits(&doc, changes.clone()) {
                            state::show_toast("warn", error.to_string());
                        }
                    }))
                })
                .collect::<Vec<_>>();
            if !rows.is_empty() {
                state::show_popup_menu(rows);
            }
        }
        Operation::Hints => {
            let rows = result
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|hint| {
                    let line = hint["position"]["line"].as_u64()? + 1;
                    let value = hint["label"].as_str().map(str::to_owned).or_else(|| {
                        hint["label"].as_array().map(|parts| {
                            parts
                                .iter()
                                .filter_map(|part| part["value"].as_str())
                                .collect::<String>()
                        })
                    })?;
                    Some(format!("{line} · {value}"))
                })
                .collect::<Vec<_>>();
            super::git_advanced::report("内联提示".into(), rows.join("\n"));
        }
    }
    Ok(())
}

fn problems() {
    let rows = global::<Diagnostics>().with_untracked(|markers| {
        markers
            .0
            .iter()
            .flat_map(|(uri, items)| {
                items.iter().map(move |item| {
                    let path = lsp::uri_path(uri)
                        .ok()
                        .and_then(|path| {
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .map(str::to_owned)
                        })
                        .unwrap_or_default();
                    let line = item["range"]["start"]["line"].as_u64().unwrap_or_default() + 1;
                    format!(
                        "{path}:{line} · {}",
                        item["message"].as_str().unwrap_or_default()
                    )
                })
            })
            .collect::<Vec<_>>()
    });
    super::git_advanced::report(
        "问题".into(),
        if rows.is_empty() {
            "当前没有诊断".into()
        } else {
            rows.join("\n")
        },
    );
}

pub(super) fn menu() -> MenuItem {
    MenuItem::submenu("语言服务", actions())
}

pub(super) fn open_menu() {
    state::show_popup_menu(actions());
}

fn actions() -> Vec<MenuItem> {
    let active = enabled();
    let mut items = vec![
        MenuItem::new("启用语言服务")
            .checked(active)
            .on_select(move || {
                app_signal().update(|s| {
                    s.cfg.extra.insert("lsp_enabled".into(), json!(!active));
                });
                state::save_config_quiet();
            }),
    ];
    if active {
        items.extend([
            MenuItem::new("补全").on_select(|| request(Operation::Completion)),
            MenuItem::new("悬停信息").on_select(|| request(Operation::Hover)),
            MenuItem::new("转到定义").on_select(|| request(Operation::Definition)),
            MenuItem::new("查找引用").on_select(|| request(Operation::References)),
            MenuItem::new("快速修复").on_select(|| request(Operation::Actions)),
            MenuItem::new("格式化文件").on_select(|| request(Operation::Format)),
            MenuItem::new("问题").on_select(problems),
            MenuItem::new("内联提示").on_select(|| request(Operation::Hints)),
            MenuItem::new("重命名符号…").on_select(|| {
                super::git_prompt::open("重命名符号", String::new(), |name| {
                    ensure!(!name.is_empty(), "输入新名称");
                    query(Operation::Rename, Some(name.to_owned()))
                });
            }),
            MenuItem::new("配置当前语言服务器…").on_select(configure),
        ]);
    }
    items
}

fn configure() {
    let Some((lang, default)) =
        active_tab().and_then(|tab| lsp::server(Path::new(&tab.file.abs_path)))
    else {
        state::show_toast("warn", "请打开支持的源代码文件");
        return;
    };
    let current = app_signal().with_untracked(|s| {
        s.cfg
            .extra
            .get("lsp_servers")
            .and_then(|value| value[lang].as_str())
            .unwrap_or(default)
            .to_owned()
    });
    super::git_prompt::open(
        format!("{lang} 语言服务器命令"),
        current,
        move |command| {
            let _ = split_args(command)?;
            app_signal().update(|state| {
                let value = state
                    .cfg
                    .extra
                    .entry("lsp_servers".into())
                    .or_insert_with(|| json!({}));
                if !value.is_object() {
                    *value = json!({});
                }
                value[lang] = json!(command);
            });
            state::save_config_quiet();
            Ok(())
        },
    );
}
