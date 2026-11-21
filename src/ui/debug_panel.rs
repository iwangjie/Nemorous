use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use nsh_base::{EditorPosition, editor};
use nsh_core::prelude::*;
use serde_json::{Value, json};

use super::workbench::{self, button};
use crate::state::{self, app_signal, current_palette};
use crate::workbench_process::ChildGroup;
use crate::workbench_protocol::Connection;
use crate::workbench_run::{RunConfig, split_args};

#[derive(Default)]
struct Runtime(Option<Session>);

struct Session {
    connection: Connection,
    _adapter: Option<ChildGroup>,
    launch: Value,
    breaks: BTreeMap<PathBuf, Vec<u32>>,
    seq: u64,
    pending: HashMap<u64, (String, Instant)>,
    thread: i64,
    frame: i64,
}

#[derive(Clone, Default)]
struct Debug {
    request: u64,
    running: bool,
    visible: bool,
    state: String,
    frames: Vec<Value>,
    variables: Vec<String>,
    console: String,
}

impl Session {
    fn connect(
        config: RunConfig,
        root: &Path,
        breaks: BTreeMap<PathBuf, Vec<u32>>,
    ) -> Result<Self> {
        let cwd = root.join(config.cwd.trim());
        let mut launch = if config.launch.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&config.launch)?
        };
        let object = launch
            .as_object_mut()
            .context("DAP Launch 需要 JSON 对象")?;
        object.entry("cwd").or_insert_with(|| json!(cwd));
        if !config.program.is_empty() {
            object
                .entry("program")
                .or_insert_with(|| json!(config.program));
        }
        if !config.program_args.is_empty() {
            object
                .entry("args")
                .or_insert(json!(split_args(&config.program_args)?));
        }
        if !config.env.is_empty() {
            object.entry("env").or_insert(json!(config.env));
        }
        let adapter = || -> Result<Command> {
            let mut command = Command::new(config.adapter.trim());
            command
                .args(split_args(&config.adapter_args)?)
                .current_dir(&cwd)
                .envs(&config.env);
            Ok(command)
        };
        let (connection, child) = if config.network.trim() == "tcp" {
            ensure!(!config.address.trim().is_empty(), "DAP 缺少 TCP 地址");
            let child = if config.adapter.trim().is_empty() {
                None
            } else {
                Some(ChildGroup::spawn(
                    adapter()?
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null()),
                )?)
            };
            let deadline = Instant::now() + Duration::from_secs(3);
            let connection = loop {
                match Connection::tcp(&config.address) {
                    Ok(connection) => break connection,
                    Err(error) if child.is_none() || Instant::now() >= deadline => {
                        return Err(error);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(60)),
                }
            };
            (connection, child)
        } else {
            ensure!(!config.adapter.trim().is_empty(), "请指定 DAP 适配器");
            (Connection::stdio(&mut adapter()?)?, None)
        };
        let mut session = Self {
            connection,
            _adapter: child,
            launch,
            breaks,
            seq: 0,
            pending: HashMap::new(),
            thread: 0,
            frame: 0,
        };
        session.request("initialize", json!({
            "clientID":"nemorous","adapterID":"nemorous","linesStartAt1":true,"columnsStartAt1":true,
            "pathFormat":"path","supportsVariableType":true,"supportsRunInTerminalRequest":false
        }))?;
        Ok(session)
    }

    fn request(&mut self, command: &str, arguments: Value) -> Result<()> {
        ensure!(self.pending.len() < 128, "DAP 在途请求过多");
        self.seq += 1;
        self.connection.send(
            json!({"type":"request","seq":self.seq,"command":command,"arguments":arguments}),
        )?;
        self.pending
            .insert(self.seq, (command.to_owned(), Instant::now()));
        Ok(())
    }

    fn set_breakpoints(&mut self, path: &Path, lines: &[u32]) -> Result<()> {
        self.request(
            "setBreakpoints",
            json!({
                "source": {"path": path},
                "breakpoints": lines.iter().map(|line| json!({"line":line})).collect::<Vec<_>>(),
                "sourceModified": false,
            }),
        )
    }

    fn configure(&mut self) -> Result<()> {
        for (path, lines) in self.breaks.clone() {
            self.set_breakpoints(&path, &lines)?;
        }
        self.request("configurationDone", json!({}))
    }

    fn handle(&mut self, message: Value, view: &mut Debug) -> Result<()> {
        let body = &message["body"];
        match message["type"].as_str() {
            Some("event") => match message["event"].as_str() {
                Some("initialized") => self.configure()?,
                Some("stopped") => {
                    self.thread = body["threadId"].as_i64().unwrap_or_default();
                    view.state = format!(
                        "已暂停 · {}",
                        body["reason"].as_str().unwrap_or("breakpoint")
                    );
                    if self.thread == 0 {
                        self.request("threads", json!({}))?;
                    } else {
                        self.request("stackTrace", json!({"threadId":self.thread,"levels":30}))?;
                    }
                }
                Some("continued") => {
                    view.state = "调试中".into();
                    view.frames.clear();
                    view.variables.clear();
                }
                Some("output") => {
                    if let Some(text) = body["output"].as_str() {
                        view.console.push_str(text);
                        if view.console.len() > 128 * 1024 {
                            let offset = view
                                .console
                                .ceil_char_boundary(view.console.len() - 128 * 1024);
                            view.console.drain(..offset);
                        }
                    }
                }
                Some("terminated" | "exited") => {
                    view.running = false;
                    view.state = "调试已结束".into();
                }
                _ => {}
            },
            Some("response") => {
                let id = message["request_seq"].as_u64().unwrap_or_default();
                let Some((command, _)) = self.pending.remove(&id) else {
                    return Ok(());
                };
                if message["success"].as_bool() != Some(true) {
                    view.state = format!(
                        "调试请求失败 · {}",
                        message["message"].as_str().unwrap_or(&command)
                    );
                    if matches!(command.as_str(), "initialize" | "launch") {
                        view.running = false;
                    }
                    return Ok(());
                }
                match command.as_str() {
                    "initialize" => self.request("launch", self.launch.clone())?,
                    "launch" if view.state == "正在连接适配器" => {
                        view.state = "调试中".into()
                    }
                    "threads" => {
                        self.thread = body["threads"][0]["id"].as_i64().unwrap_or_default();
                        if self.thread > 0 {
                            self.request(
                                "stackTrace",
                                json!({"threadId":self.thread,"levels":30}),
                            )?;
                        }
                    }
                    "stackTrace" => {
                        view.frames = body["stackFrames"].as_array().cloned().unwrap_or_default();
                        view.variables.clear();
                        if let Some(frame) = view.frames.first() {
                            self.frame = frame["id"].as_i64().unwrap_or_default();
                            self.request("scopes", json!({"frameId":self.frame}))?;
                        }
                    }
                    "scopes" => {
                        if let Some(scopes) = body["scopes"].as_array() {
                            for scope in scopes.iter().take(8) {
                                if let Some(reference) = scope["variablesReference"].as_i64() {
                                    self.request(
                                        "variables",
                                        json!({"variablesReference":reference}),
                                    )?;
                                }
                            }
                        }
                    }
                    "variables" => {
                        if let Some(vars) = body["variables"].as_array() {
                            view.variables.extend(vars.iter().take(250).map(|var| {
                                format!(
                                    "{} = {}",
                                    var["name"].as_str().unwrap_or_default(),
                                    var["value"].as_str().unwrap_or_default()
                                )
                            }));
                        }
                    }
                    _ => {}
                }
            }
            Some("request") => {
                self.seq += 1;
                self.connection.send(json!({"type":"response","seq":self.seq,"request_seq":message["seq"],
                    "command":message["command"],"success":false,"message":"不支持适配器发起的任务"}))?;
            }
            _ => {}
        }
        Ok(())
    }
}

pub(super) fn visible() -> bool {
    global::<Debug>().with(|view| view.visible)
}

pub(super) fn start(config: RunConfig) {
    stop();
    let (root, breaks) = app_signal().with_untracked(|state| {
        let breaks = state
            .cfg
            .extra
            .get("breakpoints")
            .cloned()
            .and_then(|value| serde_json::from_value::<BTreeMap<String, Vec<u32>>>(value).ok())
            .unwrap_or_default();
        let absolute = state
            .workspace_files
            .iter()
            .filter_map(|file| {
                breaks
                    .get(&file.key())
                    .map(|lines| (PathBuf::from(&file.abs_path), lines.clone()))
            })
            .collect();
        (PathBuf::from(&state.cfg.workspace_path), absolute)
    });
    let request = global::<Debug>().with_untracked(|view| view.request + 1);
    global::<Debug>().set(Debug {
        request,
        visible: true,
        state: "正在连接适配器".into(),
        ..Debug::default()
    });
    state::spawn_from_root(async move {
        let result = blocking(move || Session::connect(config, &root, breaks)).await;
        if global::<Debug>().with_untracked(|view| view.request) != request {
            return;
        }
        match result {
            Ok(Ok(session)) => {
                workbench::replace_resource(Runtime(Some(session)));
                global::<Debug>().update(|view| view.running = true);
            }
            Ok(Err(error)) => global::<Debug>().update(|view| view.state = format!("{error:#}")),
            Err(error) => global::<Debug>().update(|view| view.state = error.to_string()),
        }
    });
}

pub(super) fn stop() {
    workbench::replace_resource(Runtime::default());
    global::<Debug>().update(|view| {
        view.request += 1;
        view.running = false;
        view.visible = false;
    });
}

fn poll() {
    let mut ended = false;
    workbench::update_resource::<Runtime>(|runtime| {
        let Some(session) = &mut runtime.0 else {
            return;
        };
        let mut disconnected = false;
        for _ in 0..32 {
            global::<Debug>().update(|view| {
                let result = (|| -> Result<()> {
                    if let Some(message) = session.connection.poll()? {
                        session.handle(message, view)?;
                    } else {
                        ended = true;
                    }
                    if session
                        .pending
                        .values()
                        .any(|(_, started)| started.elapsed() > Duration::from_secs(20))
                    {
                        anyhow::bail!("调试适配器等待超时");
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    view.running = false;
                    view.state = error.to_string();
                }
                if !view.running {
                    disconnected = true;
                    ended = true;
                }
            });
            if ended {
                break;
            }
        }
        if disconnected {
            runtime.0 = None;
        }
    });
}

pub(super) fn setup() {
    let running = computed(|| global::<Debug>().with(|view| view.running));
    effect(move || {
        if running.get() {
            let timer = set_interval(Duration::from_millis(40), poll);
            cleanup(move || timer.cancel());
        }
    });
}

fn action(command: &'static str) {
    workbench::update_resource::<Runtime>(|runtime| {
        if let Some(session) = runtime.0.as_mut() {
            if let Err(error) = session.request(command, json!({"threadId":session.thread})) {
                state::show_toast("warn", error.to_string());
            }
        }
    });
}

pub(super) fn active_line_breakpoint() {
    let active = app_signal()
        .with_untracked(|state| state.code_tabs.get(state.active_code_tab_idx).cloned());
    let Some(tab) = active else {
        return;
    };
    toggle_breakpoint(&tab.file, tab.cursor.line as u32 + 1);
}

pub(super) fn toggle_breakpoint(file: &crate::git::WorkspaceFile, line: u32) {
    let mut updated = Vec::new();
    app_signal().update(|state| {
        let mut breaks = state
            .cfg
            .extra
            .get("breakpoints")
            .cloned()
            .and_then(|value| serde_json::from_value::<BTreeMap<String, Vec<u32>>>(value).ok())
            .unwrap_or_default();
        let lines = breaks.entry(file.key()).or_default();
        if let Some(index) = lines.iter().position(|saved| *saved == line) {
            lines.remove(index);
        } else {
            lines.push(line);
            lines.sort_unstable();
        }
        updated = lines.clone();
        state.cfg.extra.insert("breakpoints".into(), json!(breaks));
    });
    state::save_config_quiet();
    workbench::update_resource::<Runtime>(|runtime| {
        if let Some(session) = runtime.0.as_mut() {
            if let Err(error) = session.set_breakpoints(Path::new(&file.abs_path), &updated) {
                state::show_toast("warn", error.to_string());
            }
        }
    });
}

fn open_frame(frame: Value) {
    let Some(path) = frame["source"]["path"].as_str() else {
        return;
    };
    let position = EditorPosition::new(
        frame["line"].as_u64().unwrap_or(1).saturating_sub(1) as u32,
        frame["column"].as_u64().unwrap_or(1).saturating_sub(1) as u32,
    );
    if let Some(file) = app_signal().with_untracked(|state| {
        state
            .workspace_files
            .iter()
            .find(|file| file.abs_path == path)
            .cloned()
    }) {
        state::open_code_file(file, Some(position), true);
    }
    workbench::update_resource::<Runtime>(|runtime| {
        if let (Some(session), Some(id)) = (&mut runtime.0, frame["id"].as_i64()) {
            global::<Debug>().update(|view| view.variables.clear());
            let _ = session.request("scopes", json!({"frameId":id}));
        }
    });
}

pub(super) fn body() -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .gap(px(5.0))
        .child(
            view()
                .h(px(24.0))
                .flex_none()
                .flex_row()
                .gap(px(5.0))
                .items_center()
                .child(button("继续", "debug-continue", || action("continue")))
                .child(button("暂停", "debug-pause", || action("pause")))
                .child(button("单步跳过", "debug-next", || action("next")))
                .child(button("单步进入", "debug-step-in", || action("stepIn")))
                .child(button("单步退出", "debug-step-out", || {
                    action("stepOut")
                }))
                .child(
                    text(|| global::<Debug>().with(|view| view.state.clone()))
                        .test_id("debug-state")
                        .text_color(p.text_primary)
                        .text_size(px(11.0)),
                ),
        )
        .child(
            view()
                .flex_row()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    view()
                        .w(px(200.0))
                        .flex_none()
                        .flex_col()
                        .child(text("调用栈").text_color(p.text_muted).text_size(px(11.0)))
                        .child(scroll().flex_1().child(dynamic(move || {
                            view()
                                .flex_col()
                                .children(
                                    global::<Debug>()
                                        .with(|view| view.frames.clone())
                                        .into_iter()
                                        .map(|frame| {
                                            let label = format!(
                                                "{}:{}",
                                                frame["name"].as_str().unwrap_or_default(),
                                                frame["line"]
                                            );
                                            view()
                                                .h(px(20.0))
                                                .cursor(Cursor::Pointer)
                                                .on_press(move || open_frame(frame.clone()))
                                                .child(
                                                    text(label)
                                                        .text_color(p.text_primary)
                                                        .text_size(px(11.0)),
                                                )
                                        }),
                                )
                                .into_element()
                        }))),
                )
                .child(
                    view()
                        .flex_1()
                        .flex_col()
                        .child(text("变量").text_color(p.text_muted).text_size(px(11.0)))
                        .child(
                            editor()
                                .test_id("debug-variables")
                                .read_only(true)
                                .line_numbers(false)
                                .flex_1()
                                .value(|| {
                                    Some(
                                        global::<Debug>()
                                            .with(|view| view.variables.join("\n"))
                                            .into(),
                                    )
                                }),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .flex_col()
                        .child(
                            text("调试输出")
                                .text_color(p.text_muted)
                                .text_size(px(11.0)),
                        )
                        .child(
                            editor()
                                .test_id("debug-console")
                                .read_only(true)
                                .line_numbers(false)
                                .flex_1()
                                .value(|| {
                                    Some(global::<Debug>().with(|view| view.console.clone()).into())
                                }),
                        ),
                ),
        )
}
